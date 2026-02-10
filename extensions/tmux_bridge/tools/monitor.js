import { updateState } from "../lib/state.js";
import { wrapXml } from "../lib/utils.js";

const tool = {
    name: "tmux_monitor",
    description: "Register a background monitor for error patterns",
    parameters: {
        type: "object",
        properties: {
            id: { type: "string", description: "Session name" },
            pattern: { type: "string", description: "Regex pattern to watch for" },
            event_message: { type: "string", description: "Message to send on ingress when matched" },
            throttle_seconds: { type: "number", description: "Seconds to wait before re-triggering (default 60)" }
        },
        required: ["id", "pattern"]
    },
    execute: async (ctx, args) => {
        const { id, pattern, event_message, throttle_seconds = 60 } = args;

        // Validate regex
        try {
            new RegExp(pattern);
        } catch (e) {
            return wrapXml("error", "Invalid regex pattern: " + e.message);
        }

        await updateState(s => {
            if (!s.sessions[id]) throw new Error("Session not found");
            s.sessions[id].monitors.push({
                pattern,
                message: event_message || `Pattern /${pattern}/ matched`,
                throttle_seconds,
                throttle_until: 0,
                match_count: 0
            });
            return s;
        });

        // Resolve path to daemon
        // Since URL isn't available, we assume a standard layout relative to this file
        // tools/monitor.js -> ../monitor_daemon.js
        const currentUrl = import.meta.url;
        // e.g. file:///home/user/.zier/extensions/tmux_bridge/tools/monitor.js
        const currentPath = decodeURIComponent(currentUrl.replace("file://", ""));
        const toolsDir = currentPath.substring(0, currentPath.lastIndexOf('/'));
        const baseDir = toolsDir.substring(0, toolsDir.lastIndexOf('/'));
        const daemonPath = baseDir + "/monitor_daemon.js";

        await globalThis.zier.scheduler.register(
            "tmux_monitor_daemon",
            "*/30 * * * * *",
            daemonPath
        );

        return wrapXml("monitor_added", `Monitoring ${id} for /${pattern}/`);
    }
};

globalThis.pi.registerTool(tool);
