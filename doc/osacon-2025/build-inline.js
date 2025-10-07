import fs from 'fs';
import path from 'path';

// Read the presentation markdown
const presentationMd = fs.readFileSync('./src/slides/presentation.md', 'utf-8');

// Find the built asset files dynamically
const assetsDir = './dist/assets';
const assetFiles = fs.readdirSync(assetsDir);
const cssFile = assetFiles.find(f => f.startsWith('main-') && f.endsWith('.css'));
const jsFile = assetFiles.find(f => f.startsWith('main-') && f.endsWith('.js'));

if (!cssFile || !jsFile) {
  console.error('❌ Could not find built assets');
  process.exit(1);
}

// Read the built files
const indexHtml = fs.readFileSync('./dist/index.html', 'utf-8');
const mainCss = fs.readFileSync(path.join(assetsDir, cssFile), 'utf-8');
const mainJs = fs.readFileSync(path.join(assetsDir, jsFile), 'utf-8');

// Get build timestamp
const buildTime = new Date().toISOString();

// Create inline HTML with everything embedded
const inlineHtml = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Micromegas - Unified Observability for Video Games - OSACON 2025</title>
    <style>
${mainCss}
    </style>
</head>
<body>
    <div class="reveal">
        <div class="slides">
            <section data-markdown data-separator="^\\r?\\n---\\r?\\n$" data-separator-vertical="^\\r?\\n--\\r?\\n$">
                <textarea data-template>
${presentationMd}
                </textarea>
            </section>
        </div>
    </div>
    <script type="module">
console.log('Presentation built at: ${buildTime}');
${mainJs}
    </script>
</body>
</html>`;

// Write the inline version
fs.writeFileSync('./dist/presentation-inline.html', inlineHtml);

console.log('✅ Created dist/presentation-inline.html - fully self-contained, works with file:// protocol!');
console.log('📁 File size:', (Buffer.byteLength(inlineHtml) / 1024).toFixed(2), 'KB');