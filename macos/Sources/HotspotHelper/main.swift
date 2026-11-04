import Foundation

// Wakes the person's iPhone hotspot over Continuity through Sharing.framework's private
// SFRemoteHotspotSession, the session behind the hotspot row of macOS's own Wi-Fi menu.
// `browse` prints the iPhones nearby; `enable <name>` turns that iPhone's hotspot on and prints
// the hotspot's current name and password on stdout, a private pipe to the account service.
// Neither the password nor any callback argument is ever logged or printed anywhere else.

func emit(_ object: [String: Any]) {
    if let data = try? JSONSerialization.data(withJSONObject: object) {
        FileHandle.standardOutput.write(data)
    }
}

guard dlopen("/System/Library/PrivateFrameworks/Sharing.framework/Sharing", RTLD_NOW) != nil,
      let sessionClass = NSClassFromString("SFRemoteHotspotSession") as? NSObject.Type else {
    emit(["error": "unsupported"])
    exit(2)
}

func text(_ object: NSObject, _ key: String) -> String? {
    object.responds(to: NSSelectorFromString(key)) ? object.value(forKey: key) as? String : nil
}

func number(_ object: NSObject, _ key: String) -> Int? {
    object.responds(to: NSSelectorFromString(key)) ? (object.value(forKey: key) as? NSNumber)?.intValue : nil
}

func describe(_ device: NSObject) -> [String: Any]? {
    guard let name = text(device, "deviceName"), !name.isEmpty else { return nil }
    var out: [String: Any] = ["name": name]
    if let battery = number(device, "batteryLife") { out["battery"] = battery }
    if let signal = number(device, "signalStrength") { out["signal"] = signal }
    // networkType is a number; the device's own description spells it out ("5G", "LTE").
    let description = device.description
    if let range = description.range(of: "network type: ") {
        let value = description[range.upperBound...].prefix { $0 != "," && $0 != ">" }
        if !value.isEmpty { out["network"] = String(value) }
    }
    return out
}

final class Delegate: NSObject {
    var devices: [NSObject] = []
    var onUpdate: (([NSObject]) -> Void)?
    @objc func session(_ session: NSObject, updatedFoundDevices devices: [NSObject]) {
        self.devices = devices
        onUpdate?(devices)
    }
}

typealias Completion = @convention(block) (AnyObject?, AnyObject?) -> Void

nonisolated(unsafe) let session = sessionClass.init()
nonisolated(unsafe) let delegate = Delegate()
_ = session.perform(NSSelectorFromString("setDelegate:"), with: delegate)

func finish(_ code: Int32) -> Never {
    _ = session.perform(NSSelectorFromString("stopBrowsing"))
    exit(code)
}

// The iPhone being woken.
nonisolated(unsafe) var chosen: NSObject?

// enableRemoteHotspotForDevice answers SFRemoteHotspotInfo{name, password, channel}; the older
// enableHotspotForDevice answers the name and the password as two strings.
func enable(_ selectors: [String]) {
    guard let name = selectors.first, let device = chosen else {
        emit(["error": "no_info"])
        finish(5)
    }
    let selector = NSSelectorFromString(name)
    let rest = Array(selectors.dropFirst())
    guard session.responds(to: selector) else { return enable(rest) }
    let completion: Completion = { first, second in
        var found: (String, String)?
        if let info = first as? NSObject, let ssid = text(info, "name"), let password = text(info, "password"),
           !ssid.isEmpty, !password.isEmpty {
            found = (ssid, password)
        } else if let ssid = first as? String, let password = second as? String, !ssid.isEmpty, !password.isEmpty {
            found = (ssid, password)
        }
        let answer = found
        DispatchQueue.main.async {
            if let (ssid, password) = answer {
                emit(["ssid": ssid, "password": password])
                finish(0)
            }
            enable(rest)
        }
    }
    _ = session.perform(selector, with: device, with: completion as AnyObject)
}

let arguments = Array(CommandLine.arguments.dropFirst())
switch arguments.first {
case "browse" where arguments.count == 1:
    _ = session.perform(NSSelectorFromString("startBrowsing"))
    RunLoop.main.run(until: Date().addingTimeInterval(4))
    emit(["devices": delegate.devices.compactMap(describe)])
    finish(0)
case "enable" where arguments.count == 2:
    let wanted = arguments[1]
    nonisolated(unsafe) var requested = false
    delegate.onUpdate = { devices in
        guard !requested, let device = devices.first(where: { text($0, "deviceName") == wanted }) else { return }
        requested = true
        chosen = device
        enable(["enableRemoteHotspotForDevice:withCompletionHandler:", "enableHotspotForDevice:withCompletionHandler:"])
    }
    _ = session.perform(NSSelectorFromString("startBrowsing"))
    DispatchQueue.main.asyncAfter(deadline: .now() + 12) {
        if !requested {
            emit(["error": "not_found"])
            finish(3)
        }
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + 35) {
        emit(["error": "timeout"])
        finish(4)
    }
    RunLoop.main.run()
default:
    emit(["error": "usage"])
    exit(64)
}
