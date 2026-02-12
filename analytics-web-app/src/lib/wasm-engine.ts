/**
 * Lazy-loads the DataFusion WASM module.
 * The WASM binary is only fetched when this function is first called.
 */

let enginePromise: Promise<typeof import('micromegas-datafusion-wasm')> | null = null

export async function loadWasmEngine() {
  if (!enginePromise) {
    enginePromise = import('micromegas-datafusion-wasm')
      .then(async (mod) => {
        await mod.default() // initialize WASM
        return mod
      })
      .catch((e) => {
        enginePromise = null // allow retry on transient failures
        throw e
      })
  }
  return enginePromise
}
