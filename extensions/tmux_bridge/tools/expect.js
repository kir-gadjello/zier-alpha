import { updateState, appendEvent } from "../lib/state.js";
import { wrapXml, stripAnsi } from "../lib/utils.js";

const tool = {
    name: "tmux_expect",
    description: "Wait for a regex pattern in output, optionally sending a response.",
    parameters: {
        type: "object",
        properties: {
            id: { type: "string", description: "Session name" },
            pattern: { type: "string", description: "Regex pattern to wait for" },
            timeout: { type: "number", description: "Timeout in milliseconds (default 5000)" },
            send: { type: "string", description: "Response to send if matched (optional)" }
        },
        required: ["id", "pattern"]
    },
    execute: async (ctx, args) => {
        const { id, pattern, timeout = 5000, send } = args;

        let session;
        await updateState(s => { session = s.sessions[id]; return s; });
        if (!session) return wrapXml("error", "Session not found");

        const startTime = Date.now();
        const regex = new RegExp(pattern);

        let found = false;
        let matchedLine = "";

        while (Date.now() - startTime < timeout) {
            // Poll logs
            const res = await globalThis.zier.os.exec(["tmux", "-L", "zier", "capture-pane", "-t", session.tmux_id, "-p", "-S", "-20"]);
            if (res.code !== 0) {
                await new Promise(r => setTimeout(r, 200));
                continue;
            }

            const content = stripAnsi(res.stdout);
            const lines = content.split('\n');

            for (const line of lines.reverse()) {
                if (regex.test(line)) {
                    found = true;
                    matchedLine = line;
                    break;
                }
            }

            if (found) break;

            await new Promise(r => setTimeout(r, 200));
        }

        if (found) {
            if (send) {
                await globalThis.zier.os.exec(["tmux", "-L", "zier", "send-keys", "-t", session.tmux_id, send, "C-m"]);
                await appendEvent(id, "expect", `Matched /${pattern}/, sent: ${send}`);
            } else {
                await appendEvent(id, "expect", `Matched /${pattern}/`);
            }

            return wrapXml("expect_result",
                wrapXml("success", "true") +
                wrapXml("match", matchedLine) +
                (send ? wrapXml("action", "sent response") : "")
            );
        }

        return wrapXml("expect_result",
            wrapXml("success", "false") +
            wrapXml("error", "Timeout waiting for pattern")
        );
    }
};

globalThis.pi.registerTool(tool);
