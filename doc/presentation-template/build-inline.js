import fs from 'fs';
import path from 'path';

// Read the presentation markdown
const presentationMd = fs.readFileSync('./src/slides/presentation.md', 'utf-8');

// Read the built files
const indexHtml = fs.readFileSync('./dist/index.html', 'utf-8');
const mainCss = fs.readFileSync('./dist/assets/main-BsyY7_Pt.css', 'utf-8');
const mainJs = fs.readFileSync('./dist/assets/main-vmvRRDfA.js', 'utf-8');

// Create inline HTML with everything embedded
const inlineHtml = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Technical Presentation Template</title>
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
    <script>
${mainJs}
    </script>
</body>
</html>`;

// Write the inline version
fs.writeFileSync('./dist/presentation-inline.html', inlineHtml);

console.log('✅ Created dist/presentation-inline.html - fully self-contained, works with file:// protocol!');
console.log('📁 File size:', (Buffer.byteLength(inlineHtml) / 1024).toFixed(2), 'KB');