# Technical Presentation Template

A modern presentation template built with Reveal.js and the Ayu Dark theme, optimized for technical content with excellent code syntax highlighting.

## Features

- **Ayu Dark Theme** - Developer-friendly color scheme with excellent contrast
- **Markdown-based** - Write slides in simple markdown syntax
- **Code Highlighting** - Optimized syntax highlighting for technical presentations
- **Terminal Windows** - Styled terminal examples with Linux-style appearance
- **Responsive Design** - Works on desktop, tablet, and mobile
- **Multiple Build Options** - Development server, static build, or standalone file

## Quick Start

### Development

```bash
# Install dependencies
npm install

# Start development server with hot reload
npm run dev
```

Open http://localhost:5173 in your browser. Edit `src/slides/presentation.md` to modify slides.

### Building for Production

#### Option 1: Static Build (requires web server)
```bash
# Build static files
npm run build

# Preview production build
npm run preview
```
Deploy the `dist/` folder to any web server.

#### Option 2: Standalone File (no server required) ⭐
```bash
# Build completely self-contained HTML file
npm run build:standalone
```
This creates `dist/presentation-inline.html` - a single 1.2MB file containing everything needed. Perfect for:
- Opening directly in browser with `file://` protocol
- Sharing via email or USB drive
- Offline presentations
- No server dependencies

## File Structure

```
template/
├── src/
│   ├── slides/
│   │   └── presentation.md      # Your presentation content
│   ├── themes/
│   │   └── ayu-dark.css        # Ayu Dark theme styles
│   └── main.js                 # Reveal.js configuration
├── dist/                       # Build output
│   ├── index.html             # Standard build
│   └── presentation-inline.html # Standalone version
├── index.html                  # Development entry point
├── package.json
├── vite.config.js             # Build configuration
└── build-inline.js            # Standalone build script
```

## Editing Your Presentation

1. **Slides**: Edit `src/slides/presentation.md`
2. **Theme**: Customize colors in `src/themes/ayu-dark.css`
3. **Configuration**: Modify Reveal.js settings in `src/main.js`

### Slide Syntax

```markdown
# Slide Title
## Subtitle

---

## Bullet Points

- Point one
- Point two
- Point three

---

## Code Example

\`\`\`javascript
function hello() {
    console.log('Hello, World!');
}
\`\`\`

---

## Terminal Example

<div class="terminal-window">
  <div class="terminal-header">Terminal</div>
  <div class="terminal-content">
    <div class="prompt">npm install</div>
    <div class="success">✓ Installation complete</div>
  </div>
</div>
```

## Build Commands

| Command | Purpose |
|---------|---------|
| `npm run dev` | Development server with hot reload |
| `npm run build` | Static production build |
| `npm run build:standalone` | Self-contained HTML file |
| `npm run preview` | Preview production build |

## Browser Compatibility

- **Development & Static**: Modern browsers (Chrome, Firefox, Safari, Edge)
- **Standalone**: Any modern browser, works offline with `file://` protocol

## Deployment Options

1. **Static Hosting**: Deploy `dist/` folder to Netlify, Vercel, GitHub Pages, etc.
2. **Standalone File**: Share `dist/presentation-inline.html` directly
3. **Local Presentation**: Open `presentation-inline.html` in any browser

## Customization

### Colors
Edit CSS variables in `src/themes/ayu-dark.css`:
```css
:root {
  --color-primary: #59c2ff;    /* Blue accent */
  --color-secondary: #ffb454;  /* Orange accent */
  --color-bg-1: #0b0e14;       /* Background */
}
```

### Reveal.js Settings
Modify `src/main.js` to change transitions, controls, etc.:
```javascript
let deck = new Reveal({
  transition: 'slide',
  controls: true,
  progress: true,
  // ... more options
});
```

## License

MIT License - feel free to use for your presentations!