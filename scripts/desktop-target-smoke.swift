import Foundation

@main struct TargetSmoke {
    static func main() {
        func page(_ route: String? = nil) -> [String: Any] {
            var url=URLComponents(string:"app://-/index.html")!
            if let route {url.queryItems=[.init(name:"initialRoute",value:route)]}
            return ["type":"page","url":url.string!]
        }
        precondition(DesktopTargets.isPrimaryPage(page()))
        precondition(DesktopTargets.isPrimaryPage(page("/thread/synthetic")))
        for route in ["/avatar-overlay","/hotkey-window","/hotkey-window/test","/global-dictation",
                      "/chatgpt/quick-chat-prewarm","/chatgpt/quick-chat/synthetic","/debug"] {
            precondition(!DesktopTargets.isPrimaryPage(page(route)))
        }
        // The hidden page window Codex keeps warm is not a candidate; a page window someone opened still is.
        precondition(!DesktopTargets.isPrimaryPage(page("/space/local-page-synthetic?window=page&prewarm=1")))
        precondition(DesktopTargets.isPrimaryPage(page("/space/local-page-synthetic?window=page")))
        for address in ["app://-/detached-window.html","https://example.invalid/index.html","app://other/index.html"] {
            precondition(!DesktopTargets.isPrimaryPage(["type":"page","url":address]))
        }
        let main=DesktopTargets.Observation(id:"main",supported:true,compact:false,focused:false)
        let overlay=DesktopTargets.Observation(id:"overlay",supported:true,compact:true,focused:true)
        let other=DesktopTargets.Observation(id:"other",supported:true,compact:false,focused:false)
        precondition(DesktopTargets.select([main,overlay],previous:nil)=="main")
        precondition(DesktopTargets.select([main,overlay],previous:"overlay")==nil)
        precondition(DesktopTargets.select([main,other],previous:nil)==nil)
        precondition(DesktopTargets.select([main,other],previous:"main")=="main")
        precondition(DesktopTargets.select([main],previous:"stale")=="main")
        precondition(DesktopTargets.select([main,other],previous:"stale")==nil)
        precondition(DesktopTargets.select([.init(id:"bad",supported:false,compact:false,focused:true)],previous:nil)==nil)
        func listed(_ ids:[String]) -> [[String:Any]] {ids.map{var target=page();target["id"]=$0;return target}}
        let ids={(targets:[[String:Any]]) in targets.map{$0["id"] as? String}}
        // The main window was reopened: its old target has gone, so every primary page is probed again.
        precondition(ids(DesktopTargets.probed(listed(["main-2","other"]),previous:"main"))==["main-2","other"])
        precondition(DesktopTargets.select([.init(id:"main-2",supported:true,compact:false,focused:false)],previous:"main")=="main-2")
        precondition(ids(DesktopTargets.probed(listed(["main","other"]),previous:"main"))==["main"])
        precondition(ids(DesktopTargets.probed(listed(["main","other"]),previous:nil))==["main","other"])
        print("Primary window selected without focus; auxiliary and ambiguous targets rejected; a gone target is chosen afresh")
    }
}
