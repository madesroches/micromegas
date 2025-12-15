/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_BASE_PATH: string | undefined
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
