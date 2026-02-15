// extensions/hive/lib/tool.js
import { runAgent } from "./orchestrator.js";
import { getAgent } from "./registry.js";

export function registerHiveForkSubagent(agentNames) {
    pi.registerTool({
        name: "hive_fork_subagent",
        description: "Fork a subagent (Hive) – delegate a task to a specialized agent or create a clone",
        parameters: {
            type: "object",
            properties: {
                agent_name: {
                    type: "string",
                    description: "Name of the agent to delegate to (omit for clone mode)"
                },
                task: {
                    type: "string",
                    description: "Specific instructions for the sub-agent"
                },
                context_mode: {
                    type: "string",
                    enum: ["fresh", "fork"],
                    description: "fresh=clean state; fork=inherits conversation prefix (cache optimized). Clones always use fork."
                },
                attachments: {
                    type: "array",
                    items: { type: "string" },
                    description: "File paths to explicitly mount into sub-agent context"
                }
            },
            required: ["task"]
        },
        execute: async (_ctx, args) => {
            console.log("[Hive] EXECUTE CALLED WITH ARGS: " + JSON.stringify(args));
            const agentName = args.agent_name || "";
            const isClone = agentName === "";
            console.log(`[Hive] ${isClone ? 'Cloning' : 'Delegating to'} ${agentName || '(parent)'}...`);

            // Load Hive configuration
            const hiveConfig = pi.config.get("extensions.hive") || {};
            console.log("[Hive] Config loaded:", hiveConfig);

            // Clone‑mode pre‑checks
            if (isClone) {
                if (!hiveConfig.allow_clones) {
                    throw new Error("Cloning is disabled (extensions.hive.allow_clones = false)");
                }
                const currentCloneDepth = parseInt(zier.os.env.get("ZIER_HIVE_CLONE_DEPTH") || "0");
                const maxCloneDepth = hiveConfig.max_clone_fork_depth ?? 1;
                if (currentCloneDepth >= maxCloneDepth) {
                    throw new Error(`Max clone fork depth exceeded (${currentCloneDepth}/${maxCloneDepth})`);
                }
                // Apply userprompt prefix if configured
                if (hiveConfig.clone_userprompt_prefix) {
                    args.task = hiveConfig.clone_userprompt_prefix + args.task;
                }
            }

            try {
                console.log("[Hive] inner task to child:", args.task);
                const result = await runAgent(agentName, args.task, args.context_mode || "fresh", args.attachments || []);
                // result is the full IPC JSON object
                const meta = result.metadata;
                return `${result.content}\n<metadata>\nAgent: ${meta.agent}\nModel: ${meta.model}\nProvider: ${meta.provider}\nLatency: ${meta.latency_ms}ms\nTokens: ${meta.usage.input_tokens}/${meta.usage.output_tokens}\n</metadata>`;
            } catch (e) {
                return `Error: ${e.message}`;
            }
        }
    });
}
