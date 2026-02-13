/* tslint:disable */
/* eslint-disable */

export class WasmQueryEngine {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Deregister a single named table. Returns true if the table existed.
     */
    deregister_table(name: string): boolean;
    /**
     * Execute SQL, register result as a named table, return Arrow IPC stream bytes.
     */
    execute_and_register(sql: string, register_as: string): Promise<Uint8Array>;
    /**
     * Execute SQL, return Arrow IPC stream bytes.
     */
    execute_sql(sql: string): Promise<Uint8Array>;
    constructor();
    /**
     * Register Arrow IPC stream bytes as a named table.
     * Replaces any existing table with the same name.
     * Returns the number of rows registered.
     */
    register_table(name: string, ipc_bytes: Uint8Array): number;
    /**
     * Deregister all tables.
     */
    reset(): void;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_wasmqueryengine_free: (a: number, b: number) => void;
    readonly wasmqueryengine_deregister_table: (a: number, b: number, c: number) => [number, number, number];
    readonly wasmqueryengine_execute_and_register: (a: number, b: number, c: number, d: number, e: number) => any;
    readonly wasmqueryengine_execute_sql: (a: number, b: number, c: number) => any;
    readonly wasmqueryengine_new: () => number;
    readonly wasmqueryengine_register_table: (a: number, b: number, c: number, d: number, e: number) => [number, number, number];
    readonly wasmqueryengine_reset: (a: number) => void;
    readonly wasm_bindgen__closure__destroy__h19140d71437f7a8e: (a: number, b: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h406f660de0276fde: (a: number, b: number, c: any, d: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__hced1ae37e1679d1b: (a: number, b: number, c: any) => void;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
