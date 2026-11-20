// Rime extension entry: `rime.mode` picks the integration.
//   lsp   — `rime lsp` language server via vscode-languageclient (default):
//           warm state, check-on-type, quick fixes, server-side file watching.
//   spawn — tier-1 fallback: `rime check --format json .` per save (spawn.ts).

import * as vscode from 'vscode';
import {
    ExecuteCommandRequest,
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
} from 'vscode-languageclient/node';
import { activateSpawn, deactivateSpawn, runCheck } from './spawn';

let client: LanguageClient | undefined;
let output: vscode.OutputChannel;

function config<T>(key: string, fallback: T): T {
    return vscode.workspace.getConfiguration('rime').get<T>(key, fallback);
}

export async function activate(ctx: vscode.ExtensionContext): Promise<void> {
    output = vscode.window.createOutputChannel('Rime');
    ctx.subscriptions.push(output);
    const mode = config<string>('mode', 'lsp');
    if (mode === 'spawn') {
        activateSpawn(ctx, output);
        ctx.subscriptions.push(vscode.commands.registerCommand('rime.check', () => runCheck()));
        return;
    }
    ctx.subscriptions.push(
        vscode.commands.registerCommand('rime.check', () =>
            client?.sendRequest(ExecuteCommandRequest.type, { command: 'rime.recheck', arguments: [] })),
        // A failed start leaves `client` undefined (see startClient's catch),
        // so restart is really "try again": start fresh rather than calling
        // .restart() on a client vscode-languageclient considers never-started.
        vscode.commands.registerCommand('rime.restartServer', async () => {
            if (client) {
                await client.restart();
            } else {
                await startClient();
            }
        }),
    );
    await startClient();
}

async function startClient(): Promise<void> {
    const exe = config<string>('path', 'rime');
    const serverOptions: ServerOptions = { command: exe, args: ['lsp'] };
    const clientOptions: LanguageClientOptions = {
        documentSelector: [
            { scheme: 'file', language: 'luau' },
            { scheme: 'file', language: 'lua' },
        ],
        initializationOptions: { debounceMs: config<number>('debounceMs', 100) },
        outputChannel: output,
    };
    client = new LanguageClient('rime', 'Rime', serverOptions, clientOptions);
    try {
        await client.start();
    } catch (e) {
        void vscode.window.showErrorMessage(
            `rime: failed to start '${exe} lsp': ${String(e)} — set rime.path, or rime.mode = "spawn"`,
        );
        // vscode-languageclient leaves a failed-start client in a `startFailed`
        // state where .restart() throws; drop the reference so rime.check
        // no-ops and rime.restartServer retries via startClient() instead.
        client = undefined;
    }
}

export async function deactivate(): Promise<void> {
    deactivateSpawn();
    await client?.stop();
    client = undefined;
}
