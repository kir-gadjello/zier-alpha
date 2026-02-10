// extensions/hive/lib/ipc.js
export function readIpcResult(path) {
    try {
        const content = pi.readFile(path);
        return JSON.parse(content);
    } catch (e) {
        throw new Error(`Failed to read IPC result from ${path}: ${e.message}`);
    }
}
