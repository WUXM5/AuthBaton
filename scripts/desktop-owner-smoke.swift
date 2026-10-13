import Foundation

@main struct OwnerSmoke {
    static func main() throws {
        let processes = "100 1 /Applications/ChatGPT.app/Contents/MacOS/ChatGPT\n101 100 /private/child helper\n102 101 /private/grandchild\n200 1 /private/unrelated\n300 1 /Applications/Codex.app/Contents/MacOS/Codex"
        func listener(_ pid: Int, _ device: String = "0xabc", _ address: String = "127.0.0.1:9522") -> String {
            "p\(pid)\nf61\ntIPv4\nd\(device)\nn\(address)\n"
        }
        var passed = 0
        func check(_ listing: String, allowed: Bool, processList: String = processes) throws {
            do {
                let app = try desktopSocketOwner(listing: listing, processes: processList, port: 9522)
                precondition(allowed && app == "/Applications/ChatGPT.app")
            } catch {
                precondition(!allowed)
            }
            passed += 1
        }
        try check(listener(100), allowed: true)
        try check(listener(100) + listener(101) + listener(102), allowed: true)
        try check(listener(100) + listener(100), allowed: true)
        try check(listener(100) + listener(200), allowed: false)
        try check(listener(100) + listener(101, "0xdef"), allowed: false)
        try check(listener(100) + listener(300), allowed: false)
        try check(listener(101), allowed: false)
        try check(listener(100, "0xabc", "*:9522"), allowed: false)
        try check(listener(100) + listener(999), allowed: false)
        try check("p100\nf61\ntIPv4\nn127.0.0.1:9522", allowed: false)
        try check(listener(100) + listener(101), allowed: false, processList: "100 1 /Applications/ChatGPT.app/Contents/MacOS/ChatGPT\n101 102 /child\n102 101 /cycle")
        try check("", allowed: false)
        print("Socket ownership: \(passed) synthetic checks passed")
    }
}
