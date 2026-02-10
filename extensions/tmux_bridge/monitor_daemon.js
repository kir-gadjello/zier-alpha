import { loadState, updateState, appendEvent } from "./lib/state.js";
import { stripAnsi } from "./lib/utils.js";

async function run() {
    // 1. Locking
    let state = await loadState();
    if (state._daemon_lock && Date.now() - state._daemon_lock < 30000) {
        return; // Already running or crashed recently
    }
    await updateState(s => { s._daemon_lock = Date.now(); return s; });

    try {
        // Refresh state
        state = await loadState();

        for (const [name, session] of Object.entries(state.sessions || {})) {
            // Check if tmux session still exists
            const check = await globalThis.zier.os.exec(["tmux", "-L", "zier", "has-session", "-t", session.tmux_id]);
            if (check.code !== 0) {
                await globalThis.zier.ingress.push(`[Process Died] Session ${name} (${session.command}) exited.`);
                await appendEvent(name, "exit", "Process died");
                await updateState(s => {
                    delete s.sessions[name];
                    return s;
                });
                continue;
            }

            // Monitors
            if (session.monitors && session.monitors.length > 0) {
                const logRes = await globalThis.zier.os.exec(["tmux", "-L", "zier", "capture-pane", "-t", session.tmux_id, "-p", "-S", "-1000"]);
                if (logRes.code === 0) {
                    const logs = stripAnsi(logRes.stdout);

                    for (const mon of session.monitors) {
                        if (Date.now() < mon.throttle_until) continue;

                        const regex = new RegExp(mon.pattern);
                        const match = logs.match(regex);

                        if (match) {
                            const contextStr = match[0];
                            // Dedup: check if same context as last time
                            if (mon.last_match_context === contextStr) {
                                continue;
                            }

                            await globalThis.zier.ingress.push(
                                `[Monitor Alert] ${name}: ${mon.message}\n` +
                                `Pattern: /${mon.pattern}/\n` +
                                `Context: ...${contextStr.substring(0, 100)}...`
                            );
                            await appendEvent(name, "alert", `Matched /${mon.pattern}/`);

                            // Update state to throttle and save context
                            await updateState(s => {
                                if (s.sessions[name]) {
                                    const m = s.sessions[name].monitors.find(m => m.pattern === mon.pattern);
                                    if (m) {
                                        m.throttle_until = Date.now() + (mon.throttle_seconds || 60) * 1000;
                                        m.match_count++;
                                        m.last_match_context = contextStr;
                                    }
                                }
                                return s;
                            });
                        }
                    }
                }
            }
        }
    } finally {
        await updateState(s => { s._daemon_lock = 0; return s; });
    }
}

run().catch(err => {
    globalThis.console.log(`Monitor daemon error: ${err.message}`);
});
