import { loadState, updateState } from "../lib/state.js";
import { wrapXml, stripAnsi } from "../lib/utils.js";

const tool = {
    name: "tmux_inspect",
    description: "Inspect output and status of a tmux session",
    parameters: {
        type: "object",
        properties: {
            id: { type: "string", description: "Session name" },
            mode: { type: "string", enum: ["tail", "grep", "full_status"], description: "Inspection mode" },
            query: { type: "string", description: "Number of lines for tail (default 50) or regex for grep" }
        },
        required: ["id", "mode"]
    },
    execute: async (ctx, args) => {
        const { id, mode, query } = args;

        const state = await loadState();
        const session = state.sessions[id];

        if (!session) {
            return wrapXml("error", `Session '${id}' not found.`, { type: "not_found" });
        }

        try {
            if (mode === "tail") {
                const lines = parseInt(query || "50");
                const res = await globalThis.zier.os.exec(["tmux", "-L", "zier", "capture-pane", "-t", session.tmux_id, "-p", "-S", `-${lines}`]);
                const output = res.code === 0 ? stripAnsi(res.stdout) : `Error capturing pane: ${res.stderr}`;

                return wrapXml("inspection_result",
                    wrapXml("id", id) +
                    wrapXml("mode", "tail") +
                    wrapXml("lines", lines) +
                    wrapXml("output", output + "\n\n[... output truncated ...]")
                );
            }

            if (mode === "grep") {
                // Client-side grep for safety
                // Capture last 2000 lines
                const res = await globalThis.zier.os.exec(["tmux", "-L", "zier", "capture-pane", "-t", session.tmux_id, "-p", "-S", "-2000"]);
                if (res.code !== 0) return wrapXml("error", res.stderr);

                const content = stripAnsi(res.stdout);
                const lines = content.split('\n');
                // Basic regex check
                let regex;
                try {
                    regex = new RegExp(query);
                } catch (e) {
                    return wrapXml("error", "Invalid regex: " + e.message);
                }

                const matches = [];
                for (let i = 0; i < lines.length; i++) {
                    if (regex.test(lines[i])) {
                        matches.push({
                            line: i + 1, // Relative to capture
                            content: lines[i],
                            context: [lines[i-1], lines[i+1]].filter(l => l !== undefined)
                        });
                        if (matches.length >= 5) break;
                    }
                }

                let matchesXml = "";
                for (const m of matches) {
                    matchesXml += wrapXml("match",
                        wrapXml("line_content", m.content) +
                        (m.context.length ? wrapXml("context", m.context.join('\n')) : ""),
                        { line: m.line }
                    );
                }

                return wrapXml("inspection_result",
                    wrapXml("id", id) +
                    wrapXml("mode", "grep") +
                    wrapXml("query", query) +
                    wrapXml("matches", matchesXml) +
                    wrapXml("truncation_info", "Showing first 5 matches from last 2000 lines")
                );
            }

            if (mode === "full_status") {
                // Check if actually running
                const check = await globalThis.zier.os.exec(["tmux", "-L", "zier", "has-session", "-t", session.tmux_id]);
                const status = check.code === 0 ? "running" : "dead/exited";

                return wrapXml("inspection_result",
                    wrapXml("id", id) +
                    wrapXml("mode", "full_status") +
                    wrapXml("status", status) +
                    wrapXml("command", session.command) +
                    wrapXml("cwd", session.cwd) +
                    wrapXml("uptime", `${Math.floor((Date.now() - session.created_at) / 1000)}s`) +
                    wrapXml("monitors", session.monitors.length)
                );
            }

        } catch (e) {
            return wrapXml("error", e.message, { type: "exception" });
        }
    }
};

globalThis.pi.registerTool(tool);
