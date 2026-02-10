import { loadState } from "../lib/state.js";

export async function on_status() {
    const state = await loadState();
    const sessions = Object.entries(state.sessions || {});
    if (sessions.length === 0) return [];

    const lines = ["[ACTIVE PROCESSES (socket: zier)]"];
    for (const [name, session] of sessions) {
        const uptime = Math.floor((Date.now() - session.created_at) / 1000);
        lines.push(`- ${name}: running (${uptime}s) | monitors: ${session.monitors.length}`);
    }
    return lines;
}

globalThis.zier.hooks.on_status = on_status;
