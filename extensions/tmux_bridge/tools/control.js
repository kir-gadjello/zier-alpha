import { updateState, appendEvent } from "../lib/state.js";
import { wrapXml, stripAnsi } from "../lib/utils.js";

const tool = {
    name: "tmux_control",
    description: "Control a running tmux session (write input, signal, kill)",
    parameters: {
        type: "object",
        properties: {
            id: { type: "string", description: "Session name" },
            action: { type: "string", enum: ["write", "signal", "kill", "attach"] },
            payload: { type: "string", description: "Input to write (for write action)" }
        },
        required: ["id", "action"]
    },
    execute: async (ctx, args) => {
        const { id, action, payload } = args;

        let session;
        await updateState(s => {
            session = s.sessions[id];
            return s;
        });

        if (!session) {
            return wrapXml("error", `Session '${id}' not found.`, { type: "not_found" });
        }

        try {
            if (action === "kill") {
                await globalThis.zier.os.exec(["tmux", "-L", "zier", "kill-session", "-t", session.tmux_id]);
                await updateState(s => {
                    delete s.sessions[id];
                    return s;
                });
                await appendEvent(id, "kill", "Session killed");
                return wrapXml("control_result",
                    wrapXml("id", id) + wrapXml("action", "kill") + wrapXml("success", "true")
                );
            }

            if (action === "write") {
                if (!payload) return wrapXml("error", "Payload required for write action");
                // tmux send-keys -t <id> 'payload' C-m
                await globalThis.zier.os.exec(["tmux", "-L", "zier", "send-keys", "-t", session.tmux_id, payload, "C-m"]);

                // Wait for output
                await new Promise(r => setTimeout(r, 200));

                const logRes = await globalThis.zier.os.exec(["tmux", "-L", "zier", "capture-pane", "-t", session.tmux_id, "-p", "-S", "-5"]);
                const output = logRes.code === 0 ? stripAnsi(logRes.stdout) : "";

                await appendEvent(id, "write", payload);

                return wrapXml("control_result",
                    wrapXml("id", id) +
                    wrapXml("action", "write") +
                    wrapXml("success", "true") +
                    wrapXml("immediate_feedback", wrapXml("stdout", output))
                );
            }

            if (action === "signal") {
                // SIGINT (C-c)
                await globalThis.zier.os.exec(["tmux", "-L", "zier", "send-keys", "-t", session.tmux_id, "C-c"]);
                await appendEvent(id, "signal", "SIGINT");
                return wrapXml("control_result",
                    wrapXml("id", id) + wrapXml("action", "signal") + wrapXml("success", "true")
                );
            }

            if (action === "attach") {
                return wrapXml("control_result",
                    wrapXml("id", id) +
                    wrapXml("action", "attach") +
                    wrapXml("command", `tmux -L zier attach -t ${session.tmux_id}`)
                );
            }

        } catch (e) {
            return wrapXml("error", e.message, { type: "exception" });
        }
    }
};

globalThis.pi.registerTool(tool);
