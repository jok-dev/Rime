// Maps `rime check --format json` (schema v1) to editor-neutral diagnostic
// data. Free of the vscode module so it runs under plain `node --test`;
// extension.ts converts these to vscode.* objects.

export interface RimePosition { line: number; column: number } // 1-based; column in UTF-16 units
export interface RimeByteSpan { start: number; end: number }
export interface RimeLabel {
    primary: boolean;
    message: string;
    span: RimeByteSpan;
    start: RimePosition;
    end: RimePosition;
}
export interface RimeSuggestion { message: string; span: RimeByteSpan; replacement: string }
export interface RimeDiagnostic {
    path: string;
    code: string;
    rule: string | null;
    severity: 'error' | 'warning';
    message: string;
    conservative: boolean;
    labels: RimeLabel[];
    notes: string[];
    suggestions: RimeSuggestion[];
}
export interface RimeOutput {
    version: number;
    diagnostics: RimeDiagnostic[];
    summary: { checked: number; errors: number; warnings: number; ignored: number };
}

export interface Range { startLine: number; startColumn: number; endLine: number; endColumn: number } // 0-based
export interface MappedRelated { range: Range; message: string }
export interface MappedDiagnostic {
    severity: 'error' | 'warning';
    message: string; // rime message with notes appended
    code: string;    // "R0041 freeze-write", bare code when rule is null
    range: Range;    // primary label
    related: MappedRelated[]; // secondary labels (same file — rime spans are single-file)
}

function toRange(l: RimeLabel): Range {
    return {
        startLine: l.start.line - 1,
        startColumn: l.start.column - 1,
        endLine: l.end.line - 1,
        endColumn: l.end.column - 1,
    };
}

export function mapOutput(out: RimeOutput): Map<string, MappedDiagnostic[]> {
    const byFile = new Map<string, MappedDiagnostic[]>();
    for (const d of out.diagnostics) {
        const primary = d.labels.find(l => l.primary) ?? d.labels[0];
        // No label = no site to point at; anchor at the top of the file
        // rather than dropping the diagnostic silently.
        const range: Range = primary
            ? toRange(primary)
            : { startLine: 0, startColumn: 0, endLine: 0, endColumn: 1 };
        const message = [d.message, ...d.notes.map(n => `note: ${n}`)].join('\n');
        const mapped: MappedDiagnostic = {
            severity: d.severity,
            message,
            code: d.rule ? `${d.code} ${d.rule}` : d.code,
            range,
            related: d.labels
                .filter(l => l !== primary)
                .map(l => ({ range: toRange(l), message: l.message })),
        };
        const list = byFile.get(d.path);
        if (list) list.push(mapped);
        else byFile.set(d.path, [mapped]);
    }
    return byFile;
}
