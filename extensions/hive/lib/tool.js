// extensions/hive/lib/tool.js
import { runAgent } from "./orchestrator.js";
import { getAgent } from "./registry.js";

export function registerHiveDelegate(agentNames) {
    pi.registerTool({
        name: "hive_delegate",
        description: "Delegate a complex sub-task to a specialized agent",
        parameters: {
            type: "object",
            properties: {
                agent_name: {
                    type: "string",
                    enum: agentNames,
                    description: "Name of the agent to delegate to"
                },
                task: {
                    type: "string",
                    description: "Specific instructions for the sub-agent"
                },
                context_mode: {
                    type: "string",
                    enum: ["fresh", "fork"],
                    description: "fresh=clean state; fork=inherits conversation prefix (cache optimized)"
                },
                attachments: {
                    type: "array",
                    items: { type: "string" },
                    description: "File paths to explicitly mount into sub-agent context"
                }
            },
            required: ["agent_name", "task"]
        },
        execute: async (_ctx, args) => {
            console.log(`[Hive] Delegating to ${args.agent_name}...`);
            try {
                // Validate agent name
                if (!getAgent(args.agent_name)) {
                    throw new Error(`Agent '${args.agent_name}' not found`);
                }

                return await runAgent(args.agent_name, args.task, args.context_mode || "fresh", args.attachments || []);
            } catch (e) {
                return `Error: ${e.message}`;
            }
        }
    });
}
