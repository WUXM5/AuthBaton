// Stands in for auth-service next to the controller tests. Like the real service it holds an
// exclusive lock for its lifetime, refuses to start while another owner holds it, and exits once
// the app closes its stdin, keeping the lock a moment longer while it "stops its jobs".
// It ignores the socket path it is given: no socket, credential, Keychain or network access.
import Darwin
import Foundation

let directory = Bundle.main.executableURL!.deletingLastPathComponent().path

func record(_ event: String) {
    let line = "\(event) \(getpid())\n"
    let file = open(directory + "/stub.log", O_WRONLY | O_APPEND | O_CREAT, 0o600)
    _ = line.withCString { write(file, $0, strlen($0)) }
    close(file)
}

let lock = open(directory + "/stub.lock", O_RDWR | O_CREAT, 0o600)
guard flock(lock, LOCK_EX | LOCK_NB) == 0 else {
    record("refused")
    exit(1)
}
record("ready")
var byte: UInt8 = 0
while read(0, &byte, 1) > 0 {}
usleep(300_000)
record("exit")
exit(0)
