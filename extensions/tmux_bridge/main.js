import "./tools/spawn.js";
import "./tools/control.js";
import "./tools/inspect.js";
import "./tools/history.js";
import "./tools/expect.js";
import "./tools/monitor.js";
import "./hooks/on_status.js";
import { on_init } from "./hooks/on_init.js";

on_init().catch(err => {
    globalThis.console.log("tmux_bridge init error: " + err.message);
});
