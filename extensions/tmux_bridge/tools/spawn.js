import { updateState, appendEvent } from "../lib/state.js";
import { wrapXml, stripAnsi } from "../lib/utils.js";

const tool = {
    name: "tmux_spawn",
    description: "Spawn a background process in a new tmux session",
    parameters: {
        type: "object",
        properties: {
            command: { type: "string", description: "Shell command to run" },
            name: { type: "string", description: "Unique name for the session" },
            cwd: { type: "string", description: "Working directory (relative to workspace or project)" },
            env: { type: "object", description: "Environment variables", additionalProperties: { type: "string" } }
        },
        required: ["command", "name"]
    },
    execute: async (ctx, args) => {
        const { command, name, cwd, env } = args;

        try {
            // Check existence
            let exists = false;
            await updateState(s => {
                if (s.sessions[name]) exists = true;
                return s;
            });
            if (exists) {
                return wrapXml("error", `Session '${name}' already exists. Use a unique name or kill the existing session first.`, { type: "duplicate_name" });
            }

            const tmuxId = `zier_${name}_${Date.now()}`;

            // Spawn tmux
            const tmuxArgs = ["tmux", "-L", "zier", "new-session", "-d", "-s", tmuxId];
            if (cwd) {
                tmuxArgs.push("-c", cwd);
            }
            tmuxArgs.push(command);

            const result = await globalThis.zier.os.exec(tmuxArgs, {
                cwd: cwd,
                env: env
            });

            if (result.code !== 0) {
                return wrapXml("error", `tmux spawn failed: ${result.stderr}`, { type: "spawn_failed" });
            }

            // Wait for startup
            await new Promise(r => setTimeout(r, 500));

            // Capture startup logs
            const logRes = await globalThis.zier.os.exec(["tmux", "-L", "zier", "capture-pane", "-t", tmuxId, "-p", "-S", "-20"]);
            const startupLogs = logRes.code === 0 ? stripAnsi(logRes.stdout) : "";

            // Register in state
            await updateState(state => {
                state.sessions[name] = {
                    tmux_id: tmuxId,
                    command,
                    cwd,
                    created_at: Date.now(),
                    monitors: [],
                    last_log_idx: 0
                };
                return state;
            });

            await appendEvent(name, "spawn", `Started: ${command}`);

            return wrapXml("process_started",
                wrapXml("id", name) +
                wrapXml("tmux_session", tmuxId) +
                wrapXml("command", command) +
                wrapXml("status", "running") +
                wrapXml("startup_logs", wrapXml("stdout", startupLogs))
            );

        } catch (e) {
            return wrapXml("error", e.message, { type: "exception" });
        }
    }
};

globalThis.pi.registerTool(tool);
