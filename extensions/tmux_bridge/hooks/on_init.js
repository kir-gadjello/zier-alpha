import { loadState } from "../lib/state.js";

export async function on_init() {
    const state = await loadState();
    const hasMonitors = Object.values(state.sessions || {}).some(s => s.monitors && s.monitors.length > 0);

    if (hasMonitors) {
        // Resolve path to daemon using string manipulation since URL is not available
        const currentUrl = import.meta.url;
        const currentPath = currentUrl.replace("file://", "");
        const hooksDir = currentPath.substring(0, currentPath.lastIndexOf('/'));
        const baseDir = hooksDir.substring(0, hooksDir.lastIndexOf('/'));
        const daemonPath = baseDir + "/monitor_daemon.js";

        await globalThis.zier.scheduler.register(
            "tmux_monitor_daemon",
            "*/30 * * * * *",
            daemonPath
        );
        globalThis.console.log("Restored tmux monitor daemon");
    }
}
