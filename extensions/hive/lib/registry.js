// extensions/hive/lib/registry.js
import { registerHiveDelegate } from "./tool.js";

const agents = new Map();

export async function init() {
    console.log("[Hive] Registry initializing...");

    // Scan "agents" directory
    const dirs = ["agents", "extensions/hive/agents"];
    let loaded = 0;

    for (const dir of dirs) {
        try {
            const files = pi.fileSystem.readDir(dir);
            for (const file of files) {
                if (file.endsWith(".md")) {
                    const path = `${dir}/${file}`;
                    try {
                        const content = pi.readFile(path);
                        const agent = parseAgentFile(file, content);
                        if (agent) {
                            agents.set(agent.name, agent);
                            loaded++;
                        }
                    } catch (e) {
                        console.log(`[Hive] Failed to load agent ${path}: ${e.message}`);
                    }
                }
            }
        } catch (e) {
            // Dir probably doesn't exist or permission denied, ignore
        }
    }

    console.log(`[Hive] Loaded ${loaded} agents.`);

    // Register tool
    if (loaded > 0) {
        registerHiveDelegate(Array.from(agents.keys()));
    } else {
        console.log("[Hive] No agents loaded, skipping tool registration.");
    }
}

export function getAgent(name) {
    return agents.get(name);
}

function parseAgentFile(filename, content) {
    // Simple frontmatter parser
    const match = content.match(/^---\n([\s\S]*?)\n---/);
    if (!match) return null;

    const frontmatter = match[1];
    const config = {};

    // Parse key: value
    for (const line of frontmatter.split('\n')) {
        const parts = line.split(':');
        if (parts.length >= 2) {
            const key = parts[0].trim();
            let value = parts.slice(1).join(':').trim();

            // Remove quotes if present
            if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
                value = value.slice(1, -1);
            }

            // Parse array [a, b]
            if (value.startsWith('[') && value.endsWith(']')) {
                 const arrayContent = value.slice(1, -1);
                 config[key] = arrayContent.split(',').map(s => {
                     s = s.trim();
                     if ((s.startsWith('"') && s.endsWith('"')) || (s.startsWith("'") && s.endsWith("'"))) {
                         return s.slice(1, -1);
                     }
                     return s;
                 }).filter(s => s.length > 0);
            } else if (key === "temperature") {
                config[key] = parseFloat(value);
            } else {
                config[key] = value;
            }
        }
    }

    const name = filename.replace('.md', '');

    return {
        name,
        description: config.description || "No description",
        model: config.model,
        temperature: config.temperature,
        tools: config.tools || [],
        context_mode: config.context_mode || "fresh",
        system_prompt_append: config.system_prompt_append,
        full_content: content
    };
}
