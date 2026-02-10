const STATE_PATH = "state/tmux_sessions.json";
const LOCK_PATH = "state/tmux_sessions.json.lock";

async function ensureDir(path) {
    try {
        const dir = path.split('/').slice(0, -1).join('/');
        await globalThis.pi.fileSystem.mkdir(dir);
    } catch (e) {
        // Ignore if exists
    }
}

async function sleep(ms) {
    return new Promise(resolve => setTimeout(resolve, ms));
}

async function withLock(fn) {
    let retries = 20; // 1 second total
    while (retries > 0) {
        try {
            await ensureDir(LOCK_PATH);
            await globalThis.pi.fileSystem.writeFileExclusive(LOCK_PATH, String(Date.now()));
            break;
        } catch (error) {
            // Assume error means file exists (locked)
            retries--;
            if (retries === 0) throw new Error("Could not acquire state lock");
            await sleep(50);
        }
    }

    try {
        return await fn();
    } finally {
        try {
            await globalThis.pi.fileSystem.remove(LOCK_PATH);
        } catch {}
    }
}

export async function loadState() {
    return await withLock(async () => {
        try {
            const content = await globalThis.pi.readFile(STATE_PATH);
            return JSON.parse(content);
        } catch (error) {
            return { sessions: {}, version: 0, events: [] };
        }
    });
}

export async function saveState(state) {
    return await withLock(async () => {
        // Read current state to check version
        let current;
        try {
            const content = await globalThis.pi.readFile(STATE_PATH);
            current = JSON.parse(content);
        } catch {
            current = { version: 0 };
        }

        if (state.version !== undefined && state.version !== current.version) {
            throw new Error(`State conflict: Expected version ${state.version}, got ${current.version}`);
        }

        state.version = (current.version || 0) + 1;
        state.updated_at = Date.now();

        await ensureDir(STATE_PATH);
        await globalThis.pi.writeFile(STATE_PATH, JSON.stringify(state, null, 2));
        return state;
    });
}

export async function updateState(fn) {
    return await withLock(async () => {
        let state;
        try {
            const content = await globalThis.pi.readFile(STATE_PATH);
            state = JSON.parse(content);
        } catch {
            state = { sessions: {}, version: 0, events: [] };
        }

        const result = await fn(state);
        const newState = result || state;

        newState.version = (state.version || 0) + 1;
        newState.updated_at = Date.now();

        await ensureDir(STATE_PATH);
        await globalThis.pi.writeFile(STATE_PATH, JSON.stringify(newState, null, 2));
        return newState;
    });
}

export async function appendEvent(sessionId, type, payload) {
    return await updateState(async (state) => {
        if (!state.events) state.events = [];
        state.events.push({
            id: crypto.randomUUID(),
            timestamp: Date.now(),
            sessionId,
            type,
            payload
        });
        // Trim events if too long
        if (state.events.length > 1000) {
            state.events = state.events.slice(-800);
        }
        return state;
    });
}
