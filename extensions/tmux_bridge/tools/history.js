import { loadState } from "../lib/state.js";
import { wrapXml, parseTime, stripAnsi } from "../lib/utils.js";

const historyTool = {
    name: "tmux_history",
    description: "Query event log for a session (spawn, kill, signals, etc.)",
    parameters: {
        type: "object",
        properties: {
            id: { type: "string", description: "Session ID (optional, defaults to all)" },
            since: { type: "string", description: "Filter events since time (e.g. '1h', timestamp)" }
        }
    },
    execute: async (ctx, args) => {
        const { id, since } = args;
        const state = await loadState();

        let events = state.events || [];
        if (id) {
            events = events.filter(e => e.sessionId === id);
        }

        if (since) {
            const sinceTime = parseTime(since);
            events = events.filter(e => e.timestamp >= sinceTime);
        }

        let output = "";
        for (const e of events) {
            output += wrapXml("event",
                wrapXml("type", e.type) +
                wrapXml("payload", e.payload) +
                wrapXml("timestamp", new Date(e.timestamp).toISOString()),
                { session: e.sessionId }
            );
        }

        return wrapXml("history_result", output);
    }
};

const diffTool = {
    name: "tmux_diff",
    description: "Show status, recent events, and tail logs for a session to understand recent changes.",
    parameters: {
        type: "object",
        properties: {
            id: { type: "string", description: "Session name" },
            since: { type: "string", description: "Time window for events (default '5m')" }
        },
        required: ["id"]
    },
    execute: async (ctx, args) => {
        const { id, since = "5m" } = args;
        const state = await loadState();
        const session = state.sessions[id];

        if (!session) return wrapXml("error", "Session not found");

        // 1. Get Events
        const sinceTime = parseTime(since);
        const recentEvents = (state.events || [])
            .filter(e => e.sessionId === id && e.timestamp >= sinceTime);

        let eventOutput = "";
        for (const e of recentEvents) {
            eventOutput += wrapXml("event",
                wrapXml("type", e.type) +
                wrapXml("payload", e.payload) +
                wrapXml("timestamp", new Date(e.timestamp).toISOString())
            );
        }

        // 2. Get Logs (Tail 20)
        let logs = "";
        try {
            const logRes = await globalThis.zier.os.exec(["tmux", "-L", "zier", "capture-pane", "-t", session.tmux_id, "-p", "-S", "-20"]);
            if (logRes.code === 0) {
                logs = stripAnsi(logRes.stdout);
            }
        } catch {}

        // 3. Status
        // Check if actually running
        let status = "unknown";
        try {
            const check = await globalThis.zier.os.exec(["tmux", "-L", "zier", "has-session", "-t", session.tmux_id]);
            status = (check.code === 0) ? "running" : "dead";
        } catch {}

        return wrapXml("diff_result",
            wrapXml("status", status) +
            wrapXml("events", eventOutput || "None") +
            wrapXml("recent_logs", logs)
        );
    }
};

globalThis.pi.registerTool(historyTool);
globalThis.pi.registerTool(diffTool);
