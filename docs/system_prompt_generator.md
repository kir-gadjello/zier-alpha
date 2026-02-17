# System Prompt Generator

## Overview

Zier Alpha allows you to fully customize the system prompt used for the agent via a JavaScript/Deno script. This feature enables dynamic generation of the system prompt based on runtime context such as workspace directory, available tools, current time, and more.

## Configuration

In your `config.toml`, set the `system_prompt_script` field under the `[agent]` section to the path of your generator script:

```toml
[agent]
system_prompt_script = "path/to/generator.js"
```

The script must be readable by the Zier Alpha process.

## Script Interface

Your script should define a global function named `generateSystemPrompt`. The function receives a single argument `ctx` (a JSON object with context data) and must return a string (the system prompt). The function can be synchronous or asynchronous (returning a Promise).

```javascript
globalThis.generateSystemPrompt = (ctx) => {
  // Build a custom prompt using ctx properties
  return `You are a helpful assistant.\n\nWorkspace: ${ctx.workspace_dir}`;
};
```

### Context Object

The `ctx` object contains the following fields:

| Field | Type | Description |
|-------|------|-------------|
| `workspace_dir` | `string` | Path to the workspace directory (e.g., `~/.zier-alpha/workspace`). |
| `project_dir` | `string \| null` | Path to the project directory (if any), otherwise `null`. |
| `model` | `string` | The model identifier (e.g., `"claude-cli/opus"`). |
| `tool_names` | `string[]` | Array of available tool names (e.g., `["bash", "read_file", "write_file"]`). |
| `hostname` | `string \| null` | System hostname, if available. |
| `current_time` | `string` | Current local time formatted as `YYYY-MM-DD HH:MM:SS`. |
| `timezone` | `string \| null` | Timezone name (e.g., `"UTC"`), if determinable. |
| `skills_prompt` | `string \| null` | Pre‑built description of workspace skills, if any. |
| `status_lines` | `string[] \| null` | Array of status lines from installed extensions. |

### Return Value

The function must return a string. If you need to perform asynchronous operations (e.g., reading a file), you may return a `Promise<string>`.

**Important:** The returned string becomes the **entire system prompt**. You are responsible for including all necessary safety instructions, content boundary warnings, tool usage guidelines, and any other sections that are normally provided by the default builder. If you omit critical guardrails, the agent may become vulnerable to prompt injection or unsafe behavior.

## Example Script

`generator.js`:

```javascript
// Custom system prompt generator

globalThis.generateSystemPrompt = (ctx) => {
  const time = ctx.current_time;
  const tz = ctx.timezone ? ` (${ctx.timezone})` : '';
  const host = ctx.hostname ? ` on ${ctx.hostname}` : '';

  let prompt = `## System Prompt
You are Zier Alpha, a personal AI assistant.

**Environment**
- Workspace: ${ctx.workspace_dir}
- Project: ${ctx.project_dir || 'none'}
- Model: ${ctx.model}
- Time: ${time}${tz}${host}

**Available Tools**
${ctx.tool_names.map(name => `- ${name}`).join('\n')}

**Safety**
Do not pursue self‑preservation. Follow user instructions carefully.
When uncertain, ask clarifying questions.

**Memory**
Remember to use read_file before write_file/edit_file to check existing content.
Save important facts to MEMORY.md.

`;
  // Append skills if present
  if (ctx.skills_prompt) {
    prompt += `\n## Skills\n${ctx.skills_prompt}\n`;
  }

  return prompt;
};
```

## Security Considerations

- The script runs in a Deno sandbox with configurable capabilities (file read/write, network, env). By default, it inherits the sandbox policy of the extension system. Ensure your script does not request unnecessary permissions.
- If your generator script performs file I/O, you must declare the required paths via capability comments in the script:

```javascript
// @capability read=/path/to/allowed/file
```

See the Sandboxing documentation for details.
- The generator script executes with the same privileges as other Deno extensions. It should be treated as trusted code.

## Troubleshooting

If the agent fails to start or the system prompt appears empty, check the logs for errors from `ScriptService::evaluate_generator`. Common issues:

- Script file not found or not readable → verify path and permissions.
- `generateSystemPrompt` not defined → ensure you assign the function to `globalThis`.
- Syntax error in script → check script with `deno check`.
- Missing sandbox capabilities → add appropriate `// @capability` comments.

When the generator fails, Zier Alpha falls back to the default Rust‑built system prompt and logs an error.
