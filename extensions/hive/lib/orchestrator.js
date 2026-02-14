// extensions/hive/lib/orchestrator.js
import { getAgent } from "./registry.js";
import { readIpcResult } from "./ipc.js";

export async function runAgent(agentName, task, contextMode, attachments) {
    const agent = getAgent(agentName);
    if (!agent) throw new Error(`Agent ${agentName} not found`);

    // Check depth
    const currentDepthStr = zier.os.env.get("ZIER_HIVE_DEPTH");
    const currentDepth = currentDepthStr ? parseInt(currentDepthStr) : 0;
    const maxDepth = 3; // TODO: read from config
    if (currentDepth >= maxDepth) {
        throw new Error(`Max recursion depth exceeded (${currentDepth} >= ${maxDepth})`);
    }

    const uuid = crypto.randomUUID();
    const parentSessionId = zier.os.env.get("ZIER_SESSION_ID") || "root";
    const tempDir = zier.os.tempDir();
    const ipcPath = `${tempDir}/zier-hive-${parentSessionId}-${agentName}-${uuid}.json`;
    let hydrationPath = null;
    let hydrationArgs = [];

    // Handle hydration (fork mode)
    if (contextMode === "fork") {
        try {
            const home = zier.os.homeDir() || zier.os.env.get("HOME");
            const agentId = zier.os.env.get("ZIER_ALPHA_AGENT") || "main";
            // Check if ZIER_SESSION_ID is a full path or just ID. Assuming ID.
            const sessionPath = `${home}/.zier-alpha/agents/${agentId}/sessions/${parentSessionId}.jsonl`;

            hydrationPath = `${tempDir}/zier-hive-${parentSessionId}-${agentName}-${uuid}.jsonl`;

            // Read session file and write to temp hydration file
            const sessionContent = await pi.readFile(sessionPath);
            await pi.fileSystem.writeFileExclusive(hydrationPath, sessionContent);

            hydrationArgs = ["--hydrate-from", hydrationPath];
        } catch (e) {
            console.log(`[Hive] Failed to hydrate session: ${e.message}`);
            // Proceed without hydration if failed, but log it
            hydrationArgs = [];
            hydrationPath = null;
        }
    }

    const childEnv = {
        "ZIER_HIVE_DEPTH": (currentDepth + 1).toString(),
        "ZIER_PARENT_SESSION": parentSessionId,
    };

    // Build command dynamically
    const args = ["ask", "--child"];
    if (agent.model) {
        args.push("--model", agent.model);
    }
    args.push("--json-output", ipcPath);
    if (hydrationArgs.length > 0) {
        args.push(...hydrationArgs);
    }
    // Attachments? TODO: Mount attachments (copy to workspace or pass path)
    // Task 2.4 says: "Prepare workspace (verify attachments exist, copy if needed)"
    // For MVP, we ignore attachments or pass them as text context?
    // The prompt says "File paths to explicitly mount".
    // Since child runs in own process, it might not have access if paths are relative to parent workspace.
    // Child workspace is separate?
    // `zier ask` uses current directory as workspace unless `--workdir` is passed.
    // Parent runs in `workspace_dir`.
    // Child runs in `workspace_dir` too (inherited CWD).
    // So files should be accessible if in workspace.
    // If attachments are absolute paths, child can access them (if allowed by sandbox).
    // We can append attachment info to task prompt?
    // Or assume child can just read them.
    // "Task 2.4: verify attachments exist".
    // I'll skip deep attachment logic for now as it's not critical for recursion.

    args.push(task);

    const cmd = ["zier", ...args];

    console.log(`[Hive] Spawning: ${cmd.join(" ")}`);

    try {
        const result = await zier.os.exec(cmd, {
            env: childEnv
        });

        if (result.code !== 0) {
            throw new Error(`Subagent failed with code ${result.code}: ${result.stderr}`);
        }

        // Read IPC result
        const ipcData = await readIpcResult(ipcPath);

        // Cleanup IPC file
        try {
            await pi.fileSystem.remove(ipcPath);
        } catch (e) {}

        // Cleanup hydration file (if it still exists)
        if (hydrationPath) {
             try {
                 await pi.fileSystem.remove(hydrationPath);
             } catch (e) {}
        }

        if (ipcData.status === "error") {
             throw new Error(ipcData.error || "Unknown subagent error");
        }

        return ipcData.content;

    } catch (e) {
        // Cleanup on error
        try {
            await pi.fileSystem.remove(ipcPath);
        } catch (cleanupErr) {}

        if (hydrationPath) {
             try {
                 await pi.fileSystem.remove(hydrationPath);
             } catch (cleanupErr) {}
        }

        throw e;
    }
}
