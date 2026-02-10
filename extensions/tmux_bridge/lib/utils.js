export function stripAnsi(text) {
    return text.replace(/\x1b\[[0-9;]*[a-zA-Z]/g, '');
}

export function escapeXml(text) {
    if (!text) return "";
    return String(text)
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&apos;');
}

export function wrapXml(tag, content, attrs = {}) {
    let attrStr = "";
    for (const [key, value] of Object.entries(attrs)) {
        attrStr += ` ${key}="${escapeXml(String(value))}"`;
    }
    return `<${tag}${attrStr}>${content}</${tag}>`;
}

export function truncateWithEllipsis(text, maxLines) {
    const lines = text.split('\n');
    if (lines.length <= maxLines) return text;

    const keep = lines.slice(0, maxLines);
    const removed = lines.length - maxLines;
    return keep.join('\n') + `\n[... ${removed} lines truncated ...]`;
}

export function parseTime(input) {
    // Input could be "1h", "30m", timestamp, or ISO string
    if (!input) return Date.now();
    if (typeof input === 'number') return input;

    // Relative time (ago)
    const match = String(input).match(/^(\d+)([smhd])$/);
    if (match) {
        const val = parseInt(match[1]);
        const unit = match[2];
        const mult = { s: 1000, m: 60000, h: 3600000, d: 86400000 };
        return Date.now() - (val * mult[unit]);
    }

    // Try parse
    const parsed = Date.parse(input);
    if (!isNaN(parsed)) return parsed;

    return Date.now();
}
