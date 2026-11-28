import Foundation

@main
struct DesktopBridge {
    @MainActor static func main() async {
        do {
            let data=FileHandle.standardInput.readDataToEndOfFile()
            guard data.count<128*1024, let input=try JSONSerialization.jsonObject(with:data) as? [String:Any] else {throw Failure.invalid}
            guard let scriptURL=Bundle.module.url(forResource:"bridge",withExtension:"js") else {throw Failure.invalid}
            let script=try String(contentsOf:scriptURL,encoding:.utf8)
            let operation=input["operation"] as? String ?? ""
            if operation=="selftest" {emit(["ok":script.contains("__authManagerDesktopV1"),"protocol":1]);return}
            guard let port=input["port"] as? Int, (1024...65535).contains(port) else {throw Failure.invalid}
            guard ["probe","preflight","bind","apply","update","poll","reply","release","auto_prepare","auto_pause","auto_verify","auto_resume","auto_status","auto_cancel"].contains(operation) else {throw Failure.invalid}
            let app=try appOwning(port:port)
            let appVersion=NSDictionary(contentsOfFile:app+"/Contents/Info.plist")?["CFBundleShortVersionString"] as? String
            guard let tasksURL=Bundle.module.url(forResource:"automatic",withExtension:"js") else {throw Failure.invalid}
            let scripts=[try String(contentsOf:tasksURL,encoding:.utf8),script]
            let page:(id:String,cdp:CDP)
            switch try await attach(port:port,previous:input["target"] as? String,scripts:scripts) {
            case .refused(let code): emit(["ok":false,"code":code]);return
            case .page(let id,let cdp): page=(id,cdp)
            }
            defer {page.cdp.close()}
            let expression: String
            if operation == "preflight" {
                guard let preflightURL=Bundle.module.url(forResource:"preflight",withExtension:"js") else {throw Failure.invalid}
                expression=try String(contentsOf:preflightURL,encoding:.utf8)
            } else {
                expression=try call(operation,input["params"] ?? [:])
            }
            var target=page.id
            var result:[String:Any]
            do {
                guard let value=try await page.cdp.evaluate(expression) as? [String:Any] else {throw Failure.invalid}
                result=value
            } catch where ["apply","update"].contains(operation) {
                // Codex closes its main window when the account changes and opens a new one (26.11), so the
                // page that sent the login can be gone before the bridge answers. Read the outcome there.
                let owner=(input["params"] as? [String:Any])?["owner"] as? String ?? ""
                (target,result)=try await settled(port:port,previous:page.id,owner:owner,scripts:scripts)
            }
            result["target"]=target;result["appVersion"]=appVersion
            emit(result)
        } catch let failure as ConnectionFailure {emit(["ok":false,"code":failure.rawValue])}
        catch {emit(["ok":false,"code":"connection_failed"])}
    }
    enum Attached {case page(String,CDP), refused(String)}
    // Probes the primary pages, chooses one and installs the scripts in it.
    @MainActor static func attach(port:Int,previous:String?,scripts:[String]) async throws -> Attached {
        let url=URL(string:"http://127.0.0.1:\(port)/json/list")!
        var request=URLRequest(url:url);request.timeoutInterval=2
        let (bytes,response)=try await URLSession(configuration:.ephemeral).data(for:request)
        guard response.url==url, bytes.count<512*1024, let targets=try JSONSerialization.jsonObject(with:bytes) as? [[String:Any]] else {throw Failure.invalid}
        let candidates=targets.filter(DesktopTargets.isPrimaryPage)
        var choices:[(String,CDP)]=[]
        var observations:[DesktopTargets.Observation]=[]
        var kept:CDP?
        defer {choices.forEach{if $0.1 !== kept {$0.1.close()}}}
        for target in DesktopTargets.probed(candidates,previous:previous) {
            guard let address=target["webSocketDebuggerUrl"] as? String, let u=URL(string:address), u.scheme=="ws",
                  ["127.0.0.1","localhost"].contains(u.host ?? ""), u.port==port, u.path.hasPrefix("/devtools/page/") else {continue}
            let cdp=CDP(url:u)
            guard let id=target["id"] as? String else {cdp.close();throw Failure.invalid}
            choices.append((id,cdp))
            let probe=try await cdp.evaluate("({supported:typeof window.electronBridge?.sendMessageFromView==='function',focused:document.hasFocus(),compact:document.documentElement.classList.contains('compact-window')})") as? [String:Bool]
            observations.append(.init(id:id,supported:probe?["supported"]==true,compact:probe?["compact"]==true,focused:probe?["focused"]==true))
        }
        guard let selected=DesktopTargets.select(observations,previous:previous),
              let cdp=choices.first(where:{$0.0==selected})?.1 else {return .refused("target_unavailable")}
        do {
            for script in scripts {guard try await cdp.evaluate(script) as? Bool == true else {throw Failure.invalid}}
        } catch {return .refused("bridge_install_failed")}
        kept=cdp
        return .page(selected,cdp)
    }
    // After a login whose answer was lost: waits up to 15 seconds for a primary page to answer and
    // reports what it shows. Holding this login, it is bound and confirmed, as Codex itself never logs
    // in with external tokens; anything else is returned unconfirmed, a switch not finished.
    @MainActor static func settled(port:Int,previous:String,owner:String,scripts:[String]) async throws -> (String,[String:Any]) {
        let deadline=Date().addingTimeInterval(15)
        while true {
            if case .page(let id,let cdp)? = try? await attach(port:port,previous:previous,scripts:scripts) {
                defer {cdp.close()}
                if let probe=try? await cdp.evaluate(call("probe",[:])) as? [String:Any], probe["ok"] as? Bool == true {
                    if probe["owner"] as? String == owner, probe["mode"] as? String == "chatgptAuthTokens",
                       let bound=try? await cdp.evaluate(call("bind",["owner":owner])) as? [String:Any] {
                        return (id,bound)
                    }
                    var unconfirmed=probe;unconfirmed["confirmed"]=false
                    return (id,unconfirmed)
                }
            }
            guard Date()<deadline else {throw Failure.invalid}
            try await Task.sleep(for:.milliseconds(250))
        }
    }
    static func call(_ operation:String,_ params:Any) throws -> String {
        let payload=try JSONSerialization.data(withJSONObject:["operation":operation,"params":params])
        return "globalThis.__authManagerDesktopV1.call("+String(decoding:payload,as:UTF8.self)+")"
    }
    static func output(_ executable:String,_ args:[String]) throws -> String {
        let process=Process(),pipe=Pipe();process.executableURL=URL(fileURLWithPath:executable);process.arguments=args
        process.standardOutput=pipe;process.standardError=FileHandle.nullDevice
        try process.run();let data=pipe.fileHandleForReading.readDataToEndOfFile();process.waitUntilExit()
        guard process.terminationStatus==0 else {throw Failure.invalid}
        return String(decoding:data,as:UTF8.self).trimmingCharacters(in:.whitespacesAndNewlines)
    }
    static func appOwning(port:Int) throws -> String {
        let listing: String
        do { listing = try output("/usr/sbin/lsof",["-nP","-F","pftdn","-iTCP:\(port)","-sTCP:LISTEN"]) }
        catch { throw ConnectionFailure.portUnavailable }
        let processes = try output("/bin/ps",["-axo","pid=,ppid=,comm="])
        return try desktopSocketOwner(listing: listing, processes: processes, port: port)
    }
    static func emit(_ value:[String:Any]) {
        guard let bytes=try? JSONSerialization.data(withJSONObject:value) else {return}
        FileHandle.standardOutput.write(bytes);FileHandle.standardOutput.write(Data([10]))
    }
}
