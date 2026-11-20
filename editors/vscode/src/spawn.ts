// Tier-1 Rime integration: spawn `rime check --format json .` from the
// first workspace folder and publish into a DiagnosticCollection.
// No LSP, no code actions — tier 2 reuses the same JSON (suggestions are
// already in it, unused here).
//
// Triggers: editor saves, and a filesystem watcher for changes that never
// pass through the editor (git checkout, Rojo sync, another editor). Both
// feed one debounced scheduler; a run that is still going when the next
// change lands is killed and its output discarded — never publish results
// for a tree that has already moved on.

import * as vscode from 'vscode';
import * as cp from 'child_process';
import * as path from 'path';
import { mapOutput, RimeOutput } from './mapping';

let collection: vscode.DiagnosticCollection;
let output: vscode.OutputChannel;
// The in-flight check, if any. Killed and replaced when a new run starts.
let inflight: { child: cp.ChildProcess; generation: number } | undefined;
// Bumped per run; a child whose generation is stale on close is ignored.
let generation = 0;
let debounce: NodeJS.Timeout | undefined;

const SOURCE_GLOB = '**/*.{lua,luau}';
const CONFIG_GLOB = '**/{rime.toml,*.project.json,*.project.jsonc,.luaurc}';

export function activateSpawn(ctx: vscode.ExtensionContext, sharedOutput: vscode.OutputChannel): void {
    collection = vscode.languages.createDiagnosticCollection('rime');
    output = sharedOutput;
    ctx.subscriptions.push(collection);
    ctx.subscriptions.push(
        vscode.workspace.onDidSaveTextDocument(doc => {
            if (!/\.luau?$/i.test(doc.fileName)) return;
            if (config<boolean>('checkOnSave', true)) schedule();
        }),
    );
    // External edits never fire onDidSaveTextDocument; the watcher sees
    // them. Editor saves hit both paths — the debounce folds that into one run.
    if (config<boolean>('watchFiles', true)) {
        for (const glob of [SOURCE_GLOB, CONFIG_GLOB]) {
            const watcher = vscode.workspace.createFileSystemWatcher(glob);
            watcher.onDidChange(() => schedule());
            watcher.onDidCreate(() => schedule());
            watcher.onDidDelete(() => schedule());
            ctx.subscriptions.push(watcher);
        }
    }
    runCheck();
}

export function deactivateSpawn(): void {
    if (debounce) clearTimeout(debounce);
    inflight?.child.kill();
    inflight = undefined;
}

function config<T>(key: string, fallback: T): T {
    return vscode.workspace.getConfiguration('rime').get<T>(key, fallback);
}

// Coalesce a burst of change events (a git checkout touches hundreds of
// files) into one run, `rime.debounceMs` after the last event.
function schedule(): void {
    if (debounce) clearTimeout(debounce);
    debounce = setTimeout(() => {
        debounce = undefined;
        runCheck();
    }, Math.max(0, config<number>('debounceMs', 100)));
}

export function runCheck(): void {
    const folder = vscode.workspace.workspaceFolders?.[0];
    if (!folder) return; // no folder open; nothing to check against
    if (inflight) {
        // The tree changed under a running check: its results would describe
        // a state that no longer exists. Kill it; its close handler sees a
        // stale generation and drops everything.
        output.appendLine('rime: files changed mid-check, restarting');
        inflight.child.kill();
        inflight = undefined;
    }
    const myGeneration = ++generation;
    const cwd = folder.uri.fsPath;
    const exe = config<string>('path', 'rime');
    // cwd = workspace folder: rime.toml discovery walks up from CWD, not
    // from the check target (documented CLI gotcha).
    const child = cp.spawn(exe, ['check', '--format', 'json', '.'], { cwd });
    inflight = { child, generation: myGeneration };
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', d => (stdout += d));
    child.stderr.on('data', d => (stderr += d));
    child.on('error', err => {
        if (myGeneration !== generation) return;
        inflight = undefined;
        void vscode.window.showErrorMessage(
            `rime: failed to start '${exe}': ${err.message} — set rime.path in settings`,
        );
    });
    child.on('close', code => {
        if (myGeneration !== generation) return; // superseded — output is stale
        inflight = undefined;
        // Exit 0 = clean, 1 = diagnostics found. Anything else is a failure.
        if (code !== 0 && code !== 1) {
            output.appendLine(`rime exited ${code}`);
            output.appendLine(stderr);
            void vscode.window.showErrorMessage(
                `rime check failed (exit ${code}) — see the Rime output channel`,
            );
            return;
        }
        let parsed: RimeOutput;
        try {
            parsed = JSON.parse(stdout) as RimeOutput;
        } catch (e) {
            output.appendLine(`unparseable rime output: ${String(e)}`);
            output.appendLine(stdout.slice(0, 2000));
            void vscode.window.showErrorMessage('rime produced unparseable output — see the Rime output channel');
            return;
        }
        publish(parsed, cwd);
    });
}

function publish(out: RimeOutput, cwd: string): void {
    // clear + set run in one synchronous turn, so the editor never observes
    // an empty collection between the old results and the new.
    collection.clear();
    for (const [file, diags] of mapOutput(out)) {
        const uri = vscode.Uri.file(path.resolve(cwd, file));
        collection.set(uri, diags.map(d => {
            const range = new vscode.Range(
                d.range.startLine, d.range.startColumn,
                d.range.endLine, d.range.endColumn,
            );
            const vd = new vscode.Diagnostic(
                range,
                d.message,
                d.severity === 'error'
                    ? vscode.DiagnosticSeverity.Error
                    : vscode.DiagnosticSeverity.Warning,
            );
            vd.source = 'rime';
            vd.code = d.code;
            vd.relatedInformation = d.related.map(r => new vscode.DiagnosticRelatedInformation(
                new vscode.Location(uri, new vscode.Range(
                    r.range.startLine, r.range.startColumn,
                    r.range.endLine, r.range.endColumn,
                )),
                r.message,
            ));
            return vd;
        }));
    }
}
