// Process probes for compare-tauri.sh, compiled into its temp directory.
//
//   launch <timeout_ms> <log> <exe> [args...]  spawn, print "pid N", then "window_ms M" (-1 on timeout)
//   resp <pid>                                 pids whose responsible process is <pid>
//   cpu <pid>...                               "pid user+system-ns" for each live pid
//   windows <pid>                              on-screen layer-0 windows owned by <pid>
import CoreGraphics
import Darwin
import Foundation

typealias DisclaimFn = @convention(c) (UnsafeMutablePointer<posix_spawnattr_t?>, Int32) -> Int32
typealias ResponsibleFn = @convention(c) (pid_t) -> pid_t

let libc = dlopen(nil, RTLD_NOW)

func symbol<T>(_ name: String, _: T.Type) -> T? {
    guard let raw = dlsym(libc, name) else { return nil }
    return unsafeBitCast(raw, to: T.self)
}

func nowNs() -> UInt64 { clock_gettime_nsec_np(CLOCK_UPTIME_RAW) }

func windowCount(_ pid: pid_t) -> Int {
    guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] else {
        return 0
    }
    return list.filter { info in
        guard (info[kCGWindowOwnerPID as String] as? Int32) == pid,
              (info[kCGWindowLayer as String] as? Int) == 0,
              let bounds = info[kCGWindowBounds as String] as? [String: Double]
        else { return false }
        // A 1×1 or menu-sized helper window is not the app's window.
        return (bounds["Width"] ?? 0) >= 200 && (bounds["Height"] ?? 0) >= 200
    }.count
}

func allPids() -> [pid_t] {
    let count = proc_listallpids(nil, 0)
    guard count > 0 else { return [] }
    var pids = [pid_t](repeating: 0, count: Int(count) * 2)
    let filled = proc_listallpids(&pids, Int32(pids.count * MemoryLayout<pid_t>.size))
    return Array(pids.prefix(Int(max(filled, 0)))).filter { $0 > 0 }
}

func cpuNs(_ pid: pid_t) -> UInt64? {
    var info = rusage_info_v2()
    let result = withUnsafeMutablePointer(to: &info) { ptr in
        ptr.withMemoryRebound(to: rusage_info_t?.self, capacity: 1) { proc_pid_rusage(pid, RUSAGE_INFO_V2, $0) }
    }
    guard result == 0 else { return nil }
    // ri_*_time is mach absolute time, not nanoseconds, on Apple silicon.
    var timebase = mach_timebase_info_data_t()
    mach_timebase_info(&timebase)
    let ticks = info.ri_user_time + info.ri_system_time
    return ticks * UInt64(timebase.numer) / UInt64(timebase.denom)
}

func launch(_ args: [String]) -> Int32 {
    guard args.count >= 3, let timeout = UInt64(args[0]) else {
        FileHandle.standardError.write("launch <timeout_ms> <log> <exe> [args...]\n".data(using: .utf8)!)
        return 2
    }
    let log = args[1]
    let argv = Array(args[2...])
    var attr: posix_spawnattr_t?
    posix_spawnattr_init(&attr)
    // The app becomes its own responsible process, as a Finder launch makes it,
    // so its WebKit helpers name it rather than this script's terminal.
    let disclaimed = symbol("responsibility_spawnattrs_setdisclaim", DisclaimFn.self).map { $0(&attr, 1) == 0 } ?? false
    posix_spawnattr_setpgroup(&attr, 0)
    posix_spawnattr_setflags(&attr, Int16(POSIX_SPAWN_SETPGROUP))
    var actions: posix_spawn_file_actions_t?
    posix_spawn_file_actions_init(&actions)
    posix_spawn_file_actions_addopen(&actions, 0, "/dev/null", O_RDONLY, 0)
    posix_spawn_file_actions_addopen(&actions, 1, log, O_WRONLY | O_CREAT | O_APPEND, 0o644)
    posix_spawn_file_actions_adddup2(&actions, 1, 2)
    var cargs = argv.map { strdup($0) } + [nil]
    var pid: pid_t = 0
    let start = nowNs()
    let rc = posix_spawn(&pid, argv[0], &actions, &attr, &cargs, environ)
    guard rc == 0 else {
        print("error posix_spawn \(rc)")
        return 1
    }
    print("pid \(pid)")
    print("disclaimed \(disclaimed ? 1 : 0)")
    fflush(stdout)
    let deadline = start + timeout * 1_000_000
    while nowNs() < deadline {
        if windowCount(pid) > 0 {
            print(String(format: "window_ms %.1f", Double(nowNs() - start) / 1e6))
            return 0
        }
        if kill(pid, 0) != 0 { break }
        usleep(2000)
    }
    print("window_ms -1")
    return 0
}

let args = Array(CommandLine.arguments.dropFirst())
guard let command = args.first else { exit(2) }
let rest = Array(args.dropFirst())
switch command {
case "launch":
    exit(launch(rest))
case "resp":
    guard let target = rest.first.flatMap(pid_t.init), let responsible = symbol("responsibility_get_pid_responsible_for_pid", ResponsibleFn.self) else { exit(2) }
    for pid in allPids() where pid != target && responsible(pid) == target { print(pid) }
case "cpu":
    for pid in rest.compactMap(pid_t.init) {
        if let ns = cpuNs(pid) { print("\(pid) \(ns)") }
    }
case "windows":
    guard let pid = rest.first.flatMap(pid_t.init) else { exit(2) }
    print(windowCount(pid))
default:
    exit(2)
}
