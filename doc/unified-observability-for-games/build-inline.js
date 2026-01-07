import fs from 'fs';
import path from 'path';

// Read the presentation markdown
let presentationMd = fs.readFileSync('./src/slides/presentation.md', 'utf-8');

// Convert images to data URLs
const imageRegex = /(?:src=["']\.\/|!\[.*?\]\(\.\/)([\w-]+\.(png|jpg|jpeg|svg|gif))/g;
const matches = [...presentationMd.matchAll(imageRegex)];

for (const match of matches) {
  const imagePath = match[1];
  const fullPath = path.join('./dist', imagePath);

  if (fs.existsSync(fullPath)) {
    const imageBuffer = fs.readFileSync(fullPath);
    const mimeType = imagePath.endsWith('.svg') ? 'image/svg+xml' :
                     imagePath.endsWith('.png') ? 'image/png' :
                     imagePath.endsWith('.jpg') || imagePath.endsWith('.jpeg') ? 'image/jpeg' : 'image/gif';
    const base64 = imageBuffer.toString('base64');
    const dataUrl = `data:${mimeType};base64,${base64}`;

    // Replace in markdown
    presentationMd = presentationMd.replace(new RegExp(`\\./${imagePath}`, 'g'), dataUrl);
  } else {
    console.warn(`⚠️  Image not found: ${fullPath}`);
  }
}

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
    <title>Unified Observability for Game Teams</title>
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
