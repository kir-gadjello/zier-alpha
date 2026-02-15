// extensions/hive/lib/orchestrator.js
import { getAgent } from "./registry.js";
import { readIpcResult } from "./ipc.js";

export async function runAgent(agentName, task, contextMode, attachments) {
    const parentCtx = zier.getParentContext();
    console.log("[Hive] ParentCtx raw:", parentCtx);

    // Load Hive configuration
    const hiveConfig = pi.config.get("extensions.hive") || {};

    // Determine if this is a clone (empty agentName)
    const isClone = agentName === "";

    // Validate recursion limits
    const currentDepth = parseInt(zier.os.env.get("ZIER_HIVE_DEPTH") || "0");
    const maxHiveDepth = hiveConfig.max_depth ?? 3;
    if (currentDepth >= maxHiveDepth) {
        throw new Error(`Max Hive recursion depth exceeded (${currentDepth}/${maxHiveDepth})`);
    }

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
        // userprompt prefix is applied by the tool handler before we get here.
    }

    // Resolve effective model and tool list
    let effectiveModel;
    let effectiveTools;

    if (isClone) {
        // Clone: inherit parent's model and tools (with clone‑specific filtering)
        effectiveModel = parentCtx?.model;
        if (!effectiveModel) {
            throw new Error("Cannot clone: parent model not available");
        }
        let parentTools = parentCtx?.tools || [];
        console.log("[Hive] Parent tools before filtering:", parentTools);
        const disableList = hiveConfig.clone_disable_tools || [];
        console.log("[Hive] Disable list:", disableList);
        if (disableList.length > 0) {
            parentTools = parentTools.filter(t => !disableList.includes(t));
        }
        console.log("[Hive] Parent tools after filtering:", parentTools);
        effectiveTools = parentTools;
    } else {
        // Named agent
        const agent = getAgent(agentName);
        if (!agent) {
            throw new Error(`Agent '${agentName}' not found`);
        }

        effectiveModel = agent.model;
        if (effectiveModel === '.') {
            effectiveModel = parentCtx?.model;
        }

        effectiveTools = agent.tools;
        if (effectiveTools === '.') {
            effectiveTools = parentCtx?.tools || [];
        } else if (effectiveTools === '.no_delegate') {
            // Remove the old delegate tool name; after rename it's 'hive_fork_subagent'
            effectiveTools = (parentCtx?.tools || []).filter(t => t !== 'hive_fork_subagent');
        }
    }

    const uuid = crypto.randomUUID();
    const parentSessionId = zier.os.env.get("ZIER_SESSION_ID") || "root";
    const tempDir = zier.os.tempDir();
    const ipcPath = `${tempDir}/zier-hive-${parentSessionId}-${isClone ? "clone" : agentName}-${uuid}.json`;
    let hydrationPath = null;
    let hydrationArgs = [];

    // Clone mode forces hydration to preserve system prompt identity
    const forkMode = isClone || contextMode === "fork";
    if (forkMode) {
        const home = zier.os.homeDir() || zier.os.env.get("HOME");
        const agentId = parentCtx?.agentId || zier.os.env.get("ZIER_ALPHA_AGENT") || "main";
        const sessionPath = `${home}/.zier-alpha/agents/${agentId}/sessions/${parentSessionId}.jsonl`;

        hydrationPath = `${tempDir}/zier-hive-${parentSessionId}-${isClone ? "clone" : agentName}-${uuid}.jsonl`;

        try {
            const sessionContent = await pi.readFile(sessionPath);
            await pi.fileSystem.writeFileExclusive(hydrationPath, sessionContent);
            hydrationArgs = ["--hydrate-from", hydrationPath];
        } catch (e) {
            // For clones, hydration is critical. Fail if it fails.
            if (isClone) {
                throw new Error(`Failed to hydrate session for clone: ${e.message}`);
            }
            // For named agents (fork mode), log warning but proceed (fresh start fallback)
            console.log(`[Hive] Failed to hydrate session for fork: ${e.message}`);
            hydrationArgs = [];
            hydrationPath = null;
        }
    }

    // Compute child environment
    const childEnv = {
        "ZIER_HIVE_DEPTH": (currentDepth + 1).toString(),
        "ZIER_PARENT_SESSION": parentSessionId,
        "ZIER_CHILD_TOOLS": JSON.stringify(effectiveTools),
        // Clone depth handling
        "ZIER_HIVE_CLONE_DEPTH": (isClone ? parseInt(zier.os.env.get("ZIER_HIVE_CLONE_DEPTH") || "0") + 1 : parseInt(zier.os.env.get("ZIER_HIVE_CLONE_DEPTH") || "0")).toString(),
        "ZIER_HIVE_AGENT_NAME": isClone ? "clone" : agentName,
    };

    // Pass follow‑up for clones if configured
    if (isClone && hiveConfig.clone_sysprompt_followup) {
        childEnv["ZIER_HIVE_SYSPROMPT_FOLLOWUP"] = hiveConfig.clone_sysprompt_followup;
    }

    console.log("[Hive] Child environment:", childEnv);

    // Build command
    const args = ["ask", "--child"];
    if (parentCtx?.projectDir) {
        args.push("--workdir", parentCtx.projectDir);
    }
    if (effectiveModel) {
        args.push("--model", effectiveModel);
    }
    args.push("--json-output", ipcPath);
    if (hydrationArgs.length > 0) {
        args.push(...hydrationArgs);
    }

    args.push(task);

    const cmd = ["zier", ...args];

    console.log(`[Hive] Spawning: ${cmd.join(" ")} (tools: ${effectiveTools.length})`);

    try {
        const result = await zier.os.exec(cmd, {
            env: childEnv
        });

        // DEBUG: echo child stderr to parent logs
        if (result.stderr && result.stderr.length > 0) {
            console.log(`[Hive] CHILD STDERR:\n${result.stderr}`);
        }

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

        // Return full IPC data for parent to format
        return ipcData;

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
