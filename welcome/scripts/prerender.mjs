import { readFileSync, writeFileSync, rmSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { build } from 'vite'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const distDir = join(root, 'dist')
const ssrOutDir = join(root, 'dist-ssr')

async function main() {
  await build({
    root,
    logLevel: 'warn',
    build: {
      ssr: 'src/entry-server.tsx',
      outDir: 'dist-ssr',
      emptyOutDir: true,
      rollupOptions: {
        output: {
          entryFileNames: 'entry-server.mjs',
        },
      },
    },
  })

  const { render } = await import(
    pathToFileURL(join(ssrOutDir, 'entry-server.mjs'))
  )
  const markup = render()

  const indexPath = join(distDir, 'index.html')
  const html = readFileSync(indexPath, 'utf-8')
  const placeholder = '<div id="root"></div>'
  const occurrences = html.split(placeholder).length - 1
  if (occurrences !== 1) {
    throw new Error(
      `expected exactly one occurrence of ${JSON.stringify(placeholder)} in dist/index.html, found ${occurrences}`,
    )
  }

  const injected = html
    .split(placeholder)
    .join(`<div id="root">${markup}</div>`)

  const wordCount = injected
    .replace(/<[^>]*>/g, ' ')
    .split(/\s+/)
    .filter(Boolean).length
  const MIN_WORDS = 100
  if (wordCount < MIN_WORDS) {
    throw new Error(
      `prerendered dist/index.html has only ${wordCount} words of text, expected at least ${MIN_WORDS}`,
    )
  }

  writeFileSync(indexPath, injected)

  rmSync(ssrOutDir, { recursive: true, force: true })
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
