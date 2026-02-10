import { loadState } from "../lib/state.js";
import { wrapXml, parseTime } from "../lib/utils.js";

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
    description: "Show changes since last check (placeholder implementation)",
    parameters: {
        type: "object",
        properties: {
            id: { type: "string", description: "Session name" }
        },
        required: ["id"]
    },
    execute: async (ctx, args) => {
        return wrapXml("diff_result", "Not fully implemented: use tmux_inspect tail/grep for now.");
    }
};

globalThis.pi.registerTool(historyTool);
globalThis.pi.registerTool(diffTool);
