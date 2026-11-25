// DeviceHop: menu bar shell around the bundled `devicehop-cli` binary.
// All device work happens in that binary; this file only runs it and shows the result.

import AppKit
import Carbon
import IOKit.hid
import ServiceManagement

// MARK: - Running the helper

/// The CLI is bundled next to the app executable. It is named `devicehop-cli` because
/// `devicehop` would collide with `DeviceHop` on a case-insensitive filesystem.
let helperURL: URL? = Bundle.main.executableURL?
    .deletingLastPathComponent()
    .appendingPathComponent("devicehop-cli")

struct RunResult: Sendable {
    var status: Int32
    var out: String
    var err: String

    /// stderr and stdout together, for error dialogs.
    var message: String {
        let text = [err, out]
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
            .joined(separator: "\n")
        return text.isEmpty ? "devicehop exited with status \(status)." : text
    }
}

/// Runs the helper and waits for it. Blocking: call only from a background queue.
func runHelper(_ args: [String], timeout: TimeInterval) -> RunResult {
    guard let url = helperURL, FileManager.default.isExecutableFile(atPath: url.path) else {
        return RunResult(status: 127, out: "", err: "The bundled devicehop-cli binary is missing.")
    }
    let p = Process()
    p.executableURL = url
    p.arguments = args
    let outPipe = Pipe(), errPipe = Pipe()
    p.standardOutput = outPipe
    p.standardError = errPipe
    p.standardInput = FileHandle.nullDevice
    do { try p.run() } catch {
        return RunResult(status: 126, out: "", err: "Cannot start devicehop-cli: \(error.localizedDescription)")
    }
    DispatchQueue.global().asyncAfter(deadline: .now() + timeout) {
        if p.isRunning { p.terminate() }
    }
    // Drain stderr concurrently so neither pipe can fill up and stall the child.
    let errBox = DataBox()
    let group = DispatchGroup()
    group.enter()
    DispatchQueue.global().async {
        errBox.data = errPipe.fileHandleForReading.readDataToEndOfFile()
        group.leave()
    }
    let outData = outPipe.fileHandleForReading.readDataToEndOfFile()
    group.wait()
    p.waitUntilExit()
    return RunResult(
        status: p.terminationStatus,
        out: String(decoding: outData, as: UTF8.self),
        err: String(decoding: errBox.data, as: UTF8.self))
}

final class DataBox: @unchecked Sendable { var data = Data() }

// MARK: - Parsing `list --porcelain`

struct HostInfo: Sendable, Equatable {
    var number: Int
    var paired: Bool
    var name: String
}

struct DeviceInfo: Sendable, Equatable {
    var name: String
    var hostCount = 0
    var current = 0
    var hosts: [HostInfo] = []
    var error: String?

    var currentName: String {
        if let h = hosts.first(where: { $0.number == current }), !h.name.isEmpty { return h.name }
        return "host \(current)"
    }
}

func parsePorcelain(_ text: String) -> [DeviceInfo] {
    var devices: [DeviceInfo] = []
    for line in text.split(separator: "\n", omittingEmptySubsequences: true) {
        let f = line.split(separator: "\t", omittingEmptySubsequences: false).map(String.init)
        switch f.first {
        case "device" where f.count >= 4:
            devices.append(DeviceInfo(name: f[1], hostCount: Int(f[2]) ?? 0, current: Int(f[3]) ?? 0))
        case "host" where f.count >= 3 && !devices.isEmpty:
            devices[devices.count - 1].hosts.append(
                HostInfo(number: Int(f[1]) ?? 0, paired: f[2] == "1", name: f.count > 3 ? f[3] : ""))
        case "error" where f.count >= 3:
            devices.append(DeviceInfo(name: f[1], error: f[2]))
        default:
            break
        }
    }
    return devices
}

enum ListState: Sendable, Equatable {
    case loading
    case failed(String)
    case devices([DeviceInfo])
}

// MARK: - Follow child process

/// Keeps `devicehop-cli follow <target>` running; restarts it 5 s after an unexpected exit.
/// All state is confined to `queue`.
final class FollowSupervisor: @unchecked Sendable {
    private let queue = DispatchQueue(label: "dev.devicehop.follow")
    private var process: Process?
    private var target: String?
    private var generation = 0

    func set(target: String?) {
        queue.async {
            self.generation += 1
            self.stop()
            self.target = target
            self.start()
        }
    }

    /// Synchronous: returns once the child is gone. Used when quitting.
    func shutdown() {
        queue.sync {
            self.generation += 1
            self.target = nil
            self.stop()
        }
    }

    private func stop() {
        guard let p = process else { return }
        process = nil
        if p.isRunning {
            p.terminate()
            p.waitUntilExit()
        }
    }

    private func start() {
        guard let target, let url = helperURL, FileManager.default.isExecutableFile(atPath: url.path) else { return }
        let gen = generation
        let p = Process()
        p.executableURL = url
        p.arguments = ["follow", target]
        p.standardInput = FileHandle.nullDevice
        p.standardOutput = FileHandle.nullDevice
        p.standardError = FileHandle.nullDevice
        p.terminationHandler = { [weak self] _ in
            guard let self else { return }
            self.queue.async {
                // A deliberate stop bumps `generation`; only react to unexpected exits.
                guard self.generation == gen else { return }
                self.process = nil
                self.queue.asyncAfter(deadline: .now() + 5) {
                    if self.generation == gen, self.process == nil { self.start() }
                }
            }
        }
        do {
            try p.run()
            process = p
        } catch {
            queue.asyncAfter(deadline: .now() + 5) {
                if self.generation == gen, self.process == nil { self.start() }
            }
        }
    }
}

// MARK: - Hotkey (Carbon)

private let hotKeyHandler: EventHandlerUPP = { _, _, _ in
    Task { @MainActor in AppController.shared.hotkeyPressed() }
    return noErr
}

@MainActor
func registerHotKey() {
    var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
    InstallEventHandler(GetApplicationEventTarget(), hotKeyHandler, 1, &spec, nil, nil)
    let id = EventHotKeyID(signature: 0x4348_4F50 /* 'CHOP' */, id: 1)
    var ref: EventHotKeyRef?
    let status = RegisterEventHotKey(
        UInt32(kVK_ANSI_H), UInt32(controlKey | optionKey | cmdKey), id, GetApplicationEventTarget(), 0, &ref)
    if status != noErr { NSLog("DeviceHop: RegisterEventHotKey failed (\(status))") }
}

// MARK: - App

@MainActor
func showAlert(_ title: String, _ text: String) {
    NSApp.activate(ignoringOtherApps: true)
    let alert = NSAlert()
    alert.alertStyle = .warning
    alert.messageText = title
    alert.informativeText = text.count > 1500 ? String(text.prefix(1500)) + "…" : text
    alert.runModal()
}

@MainActor
final class AppController: NSObject, NSApplicationDelegate, NSMenuDelegate {
    static let shared = AppController()

    private let defaultsKey = "followTarget"
    private let statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
    private let menu = NSMenu()
    private let commands = DispatchQueue(label: "dev.devicehop.commands")
    private let follow = FollowSupervisor()
    private var refreshToken = 0
    /// Last known device state: shown at once when the menu opens, then refreshed.
    private var cached: ListState?
    private var menuIsOpen = false
    private var signalSources: [DispatchSourceSignal] = []

    private var followTarget: String? {
        get {
            let v = UserDefaults.standard.string(forKey: defaultsKey)
            return (v?.isEmpty ?? true) ? nil : v
        }
        set { UserDefaults.standard.set(newValue, forKey: defaultsKey) }
    }

    private var inputMonitoringGranted: Bool {
        IOHIDCheckAccess(kIOHIDRequestTypeListenEvent) == kIOHIDAccessTypeGranted
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        menu.autoenablesItems = false
        menu.delegate = self
        statusItem.menu = menu
        if let button = statusItem.button {
            if let url = Bundle.main.url(forResource: "MenuBarIcon", withExtension: "pdf"),
               let img = NSImage(contentsOf: url) {
                img.size = NSSize(width: 18, height: 18)
                img.isTemplate = true
                img.accessibilityDescription = "DeviceHop"
                button.image = img
            } else if let img = NSImage(systemSymbolName: "arrow.left.arrow.right", accessibilityDescription: "DeviceHop") {
                img.isTemplate = true
                button.image = img
            } else {
                button.title = "⇄"
            }
        }
        // Make the app show up in the Input Monitoring list.
        _ = IOHIDRequestAccess(kIOHIDRequestTypeListenEvent)
        registerHotKey()
        refresh()
        installSignalHandlers()
        follow.set(target: followTarget)
        render(.loading)
    }

    func applicationWillTerminate(_ notification: Notification) {
        follow.shutdown()
    }

    /// `pkill` and Ctrl-C send signals that would skip applicationWillTerminate and orphan the child.
    private func installSignalHandlers() {
        for sig in [SIGTERM, SIGINT, SIGHUP] {
            signal(sig, SIG_IGN)
            let src = DispatchSource.makeSignalSource(signal: sig, queue: .main)
            src.setEventHandler { MainActor.assumeIsolated { NSApp.terminate(nil) } }
            src.resume()
            signalSources.append(src)
        }
    }

    // MARK: Menu

    func menuWillOpen(_ menu: NSMenu) {
        menuIsOpen = true
        render(cached ?? .loading)
        refresh()
    }

    func menuDidClose(_ menu: NSMenu) {
        menuIsOpen = false
    }

    /// Asks the devices for their state in the background and updates the cache.
    /// The open menu is redrawn only when something actually changed. Devices are
    /// queried at launch, when the menu opens and after a switch, never on a timer.
    private func refresh(after delay: TimeInterval = 0) {
        refreshToken &+= 1
        let token = refreshToken
        commands.asyncAfter(deadline: .now() + delay) {
            let r = runHelper(["list", "--porcelain"], timeout: 20)
            let devices = parsePorcelain(r.out)
            let state: ListState = devices.isEmpty ? .failed(r.message) : .devices(devices)
            Task { @MainActor in
                let app = AppController.shared
                guard app.refreshToken == token else { return }
                let changed = app.cached != state
                app.cached = state
                if app.menuIsOpen && changed { app.render(state) }
            }
        }
    }

    private func disabled(_ title: String) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        item.isEnabled = false
        return item
    }

    private func action(_ title: String, _ sel: Selector, represented: Any? = nil) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: sel, keyEquivalent: "")
        item.target = self
        item.representedObject = represented
        return item
    }

    /// Distinct non-empty paired host names across all devices, in first-seen order.
    private func hostNames(_ devices: [DeviceInfo]) -> [String] {
        var seen = Set<String>(), out: [String] = []
        for d in devices where d.error == nil {
            for h in d.hosts where h.paired && !h.name.isEmpty && seen.insert(h.name).inserted {
                out.append(h.name)
            }
        }
        return out
    }

    private func render(_ state: ListState) {
        menu.removeAllItems()

        if !inputMonitoringGranted {
            menu.addItem(action("Grant Input Monitoring…", #selector(grantInputMonitoring)))
            menu.addItem(disabled("Restart DeviceHop after granting"))
            menu.addItem(.separator())
        }

        var names: [String] = []
        switch state {
        case .loading:
            menu.addItem(disabled("Loading…"))
        case .failed(let message):
            menu.addItem(disabled(String(message.split(separator: "\n").first ?? "No devices found").prefix(80) + ""))
        case .devices(let devices):
            names = hostNames(devices)
            for d in devices {
                if let e = d.error {
                    menu.addItem(disabled("\(d.name) — \(e.split(separator: "\n").first.map(String.init) ?? e)".prefix(90) + ""))
                } else {
                    menu.addItem(disabled("\(d.name) — \(d.currentName)"))
                }
            }
            let working = devices.filter { $0.error == nil }
            let candidates = names.filter { n in
                !working.allSatisfy { d in d.hosts.first(where: { $0.number == d.current })?.name == n }
            }
            if !candidates.isEmpty {
                menu.addItem(.separator())
                menu.addItem(disabled("Switch all devices to"))
                for n in candidates { menu.addItem(action(n, #selector(switchTo(_:)), represented: n)) }
            }
        }

        // Follow mode
        menu.addItem(.separator())
        let followItem = NSMenuItem(title: "Follow mode: \(followTarget ?? "Off")", action: nil, keyEquivalent: "")
        let sub = NSMenu()
        sub.autoenablesItems = false
        let off = action("Off", #selector(setFollow(_:)), represented: "")
        off.state = followTarget == nil ? .on : .off
        sub.addItem(off)
        var choices = names
        if let t = followTarget, !choices.contains(t) { choices.append(t) }
        for n in choices {
            let item = action(n, #selector(setFollow(_:)), represented: n)
            item.state = followTarget == n ? .on : .off
            sub.addItem(item)
        }
        followItem.submenu = sub
        menu.addItem(followItem)
        menu.addItem(disabled("Press Easy-Switch on one device; the others follow."))

        menu.addItem(.separator())
        if let t = followTarget {
            menu.addItem(disabled("Hotkey: ⌃⌥⌘H switches to \(t)"))
        } else {
            menu.addItem(disabled("Hotkey: ⌃⌥⌘H (choose a follow target first)"))
        }

        let login = action("Launch at login", #selector(toggleLogin))
        login.state = SMAppService.mainApp.status == .enabled ? .on : .off
        menu.addItem(login)

        menu.addItem(.separator())
        menu.addItem(action("Quit DeviceHop", #selector(quit)))
    }

    // MARK: Actions

    private func performSwitch(to name: String) {
        commands.async {
            let r = runHelper(["switch", name], timeout: 30)
            Task { @MainActor in
                if r.status != 0 { showAlert("Could not switch to “\(name)”", r.message) }
                // Devices that left take a moment to disappear from the system.
                AppController.shared.refresh(after: 1.5)
            }
        }
    }

    @objc private func switchTo(_ sender: NSMenuItem) {
        if let name = sender.representedObject as? String { performSwitch(to: name) }
    }

    @objc private func setFollow(_ sender: NSMenuItem) {
        let name = (sender.representedObject as? String) ?? ""
        followTarget = name.isEmpty ? nil : name
        follow.set(target: followTarget)
    }

    func hotkeyPressed() {
        if let t = followTarget { performSwitch(to: t) } else { NSSound.beep() }
    }

    @objc private func toggleLogin() {
        do {
            if SMAppService.mainApp.status == .enabled {
                try SMAppService.mainApp.unregister()
            } else {
                try SMAppService.mainApp.register()
            }
        } catch {
            showAlert("Launch at login failed", error.localizedDescription)
        }
    }

    @objc private func grantInputMonitoring() {
        _ = IOHIDRequestAccess(kIOHIDRequestTypeListenEvent)
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent") {
            NSWorkspace.shared.open(url)
        }
    }

    @objc private func quit() {
        follow.shutdown()
        NSApp.terminate(nil)
    }
}

// MARK: - Main

MainActor.assumeIsolated {
    let app = NSApplication.shared
    app.setActivationPolicy(.accessory)
    app.delegate = AppController.shared
    app.run()
}
