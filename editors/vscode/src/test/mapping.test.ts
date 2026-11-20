import test from 'node:test';
import * as assert from 'node:assert/strict';
import { mapOutput, RimeOutput } from '../mapping';

const pos = (line: number, column: number) => ({ line, column });
const label = (primary: boolean, message: string, s: [number, number], e: [number, number]) => ({
    primary,
    message,
    span: { start: 0, end: 1 },
    start: pos(s[0], s[1]),
    end: pos(e[0], e[1]),
});

const output = (diagnostics: RimeOutput['diagnostics']): RimeOutput => ({
    version: 1,
    diagnostics,
    summary: { checked: 1, errors: 1, warnings: 0, ignored: 0 },
});

test('primary label becomes the range, 1-based to 0-based', () => {
    const out = output([{
        path: 'src\\a.luau',
        code: 'R0041',
        rule: 'freeze-write',
        severity: 'error',
        message: 'cannot write to a frozen table',
        conservative: false,
        labels: [label(true, 'write here', [4, 1], [4, 6])],
        notes: [],
        suggestions: [],
    }]);
    const byFile = mapOutput(out);
    const diags = byFile.get('src\\a.luau');
    assert.ok(diags && diags.length === 1);
    assert.deepEqual(diags[0].range, { startLine: 3, startColumn: 0, endLine: 3, endColumn: 5 });
    assert.equal(diags[0].severity, 'error');
    assert.equal(diags[0].code, 'R0041 freeze-write');
});

test('secondary labels become related information', () => {
    const out = output([{
        path: 'a.luau',
        code: 'R0041',
        rule: 'freeze-write',
        severity: 'error',
        message: 'cannot write to a frozen table',
        conservative: false,
        labels: [
            label(false, 'frozen here', [2, 1], [2, 20]),
            label(true, 'write here', [4, 1], [4, 6]),
        ],
        notes: [],
        suggestions: [],
    }]);
    const d = mapOutput(out).get('a.luau')![0];
    // Primary found even when not first.
    assert.equal(d.range.startLine, 3);
    assert.equal(d.related.length, 1);
    assert.equal(d.related[0].message, 'frozen here');
    assert.equal(d.related[0].range.startLine, 1);
});

test('notes append to the message; null rule falls back to bare code', () => {
    const out = output([{
        path: 'a.luau',
        code: 'R9999',
        rule: null,
        severity: 'warning',
        message: 'top line',
        conservative: true,
        labels: [label(true, 'here', [1, 1], [1, 2])],
        notes: ['why it matters', 'how to fix'],
        suggestions: [],
    }]);
    const d = mapOutput(out).get('a.luau')![0];
    assert.equal(d.message, 'top line\nnote: why it matters\nnote: how to fix');
    assert.equal(d.code, 'R9999');
});

test('label-less diagnostics anchor at 0:0; multiple files group', () => {
    const base = {
        code: 'R0013', rule: 'unused-local' as string | null, severity: 'warning' as const,
        message: 'm', conservative: false, notes: [], suggestions: [],
    };
    const out = output([
        { ...base, path: 'a.luau', labels: [] },
        { ...base, path: 'b.luau', labels: [label(true, 'x', [1, 1], [1, 2])] },
        { ...base, path: 'b.luau', labels: [label(true, 'y', [2, 1], [2, 2])] },
    ]);
    const byFile = mapOutput(out);
    // No label = no site to point at, but the diagnostic must still show —
    // hiding an occurrence makes it unnavigable AND invisible.
    const anchored = byFile.get('a.luau')!;
    assert.equal(anchored.length, 1);
    assert.deepEqual(anchored[0].range, { startLine: 0, startColumn: 0, endLine: 0, endColumn: 1 });
    assert.equal(byFile.get('b.luau')!.length, 2);
});
