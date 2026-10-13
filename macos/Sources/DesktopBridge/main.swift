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
            let url=URL(string:"http://127.0.0.1:\(port)/json/list")!
            var request=URLRequest(url:url);request.timeoutInterval=2
            let (bytes,response)=try await URLSession(configuration:.ephemeral).data(for:request)
            guard response.url==url, bytes.count<512*1024, let targets=try JSONSerialization.jsonObject(with:bytes) as? [[String:Any]] else {throw Failure.invalid}
            let candidates=targets.filter(DesktopTargets.isPrimaryPage)
            let previous=input["target"] as? String
            var choices:[([String:Any],CDP)]=[]
            var observations:[DesktopTargets.Observation]=[]
            defer {choices.forEach{$0.1.close()}}
            for target in candidates {
                if let previous, target["id"] as? String != previous {continue}
                guard let address=target["webSocketDebuggerUrl"] as? String, let u=URL(string:address), u.scheme=="ws",
                      ["127.0.0.1","localhost"].contains(u.host ?? ""), u.port==port, u.path.hasPrefix("/devtools/page/") else {continue}
                let cdp=CDP(url:u)
                choices.append((target,cdp))
                guard let id=target["id"] as? String else {throw Failure.invalid}
                let probe=try await cdp.evaluate("({supported:typeof window.electronBridge?.sendMessageFromView==='function',focused:document.hasFocus(),compact:document.documentElement.classList.contains('compact-window')})") as? [String:Bool]
                observations.append(.init(id:id,supported:probe?["supported"]==true,compact:probe?["compact"]==true,focused:probe?["focused"]==true))
            }
            guard let selected=DesktopTargets.select(observations,previous:previous),
                  let (target,cdp)=choices.first(where:{$0.0["id"] as? String == selected}) else {emit(["ok":false,"code":"target_unavailable"]);return}
            guard let tasksURL=Bundle.module.url(forResource:"automatic",withExtension:"js") else {throw Failure.invalid}
            do {
                guard try await cdp.evaluate(String(contentsOf:tasksURL,encoding:.utf8)) as? Bool == true,
                      try await cdp.evaluate(script) as? Bool == true else {throw Failure.invalid}
            } catch {emit(["ok":false,"code":"bridge_install_failed"]);return}
            if operation == "auto_prepare" {
                guard let uiURL=Bundle.module.url(forResource:"selective-ui",withExtension:"js") else {throw Failure.invalid}
                try await SelectiveUI.install(on:cdp,script:try String(contentsOf:uiURL,encoding:.utf8),mapping:try SelectiveSymbols.read(app:app))
            }
            let payload=try JSONSerialization.data(withJSONObject:["operation":operation,"params":input["params"] ?? [:]])
            let expression: String
            if operation == "preflight" {
                guard let preflightURL=Bundle.module.url(forResource:"preflight",withExtension:"js") else {throw Failure.invalid}
                expression=try String(contentsOf:preflightURL,encoding:.utf8)
            } else {
                expression="globalThis.__authManagerDesktopV1.call("+String(decoding:payload,as:UTF8.self)+")"
            }
            guard var result=try await cdp.evaluate(expression) as? [String:Any] else {throw Failure.invalid}
            if operation == "preflight", result["ok"] as? Bool == true {
                guard let uiURL=Bundle.module.url(forResource:"selective-ui",withExtension:"js") else {throw Failure.invalid}
                try await SelectiveUI.install(on:cdp,script:try String(contentsOf:uiURL,encoding:.utf8),mapping:try SelectiveSymbols.read(app:app))
                if let validation=try await cdp.evaluate("(()=>{try{return {ok:globalThis.__authManagerSelectiveUIV1.validate()}}catch(e){return {ok:false,code:e.message==='ui_scope_unsupported'?'ui_scope_unsupported':'ui_adapter_unavailable'}}})()") as? [String:Any], validation["ok"] as? Bool != true {result=validation}
            }
            result["target"]=target["id"];result["appVersion"]=appVersion
            emit(result)
        } catch let failure as UIAdapterFailure {emit(["ok":false,"code":failure.rawValue])}
        catch let failure as ConnectionFailure {emit(["ok":false,"code":failure.rawValue])}
        catch {emit(["ok":false,"code":"connection_failed"])}
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
