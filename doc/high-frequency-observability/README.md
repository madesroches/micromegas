# High-Frequency Observability

Micromegas presentation: "Cost-Efficient Telemetry at Scale"

**Live version:** https://madesroches.github.io/micromegas/high-frequency-observability/

Built with Reveal.js, Ayu Dark theme, and Mermaid diagrams.

## Features

- **Ayu Dark Theme** - Developer-friendly color scheme optimized for code
- **Mermaid Diagrams** - Architecture flow diagrams for ingestion and analytics
- **Code Examples** - Real Rust and Unreal Engine instrumentation code
- **Vertical Navigation** - Code examples nested under Stage 1 (press down to navigate)
- **Markdown-based** - Simple markdown syntax for slides
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
high-frequency-observability/
├── src/
│   ├── slides/
│   │   └── presentation.md      # Presentation content
│   ├── themes/
│   │   └── ayu-dark.css        # Ayu Dark theme (bold text in blue)
│   ├── media/                  # Screenshots (Grafana, Perfetto)
│   └── main.js                 # Reveal.js + Mermaid plugin config
├── dist/                       # Build output (gitignored)
├── index.html                  # Development entry point
├── presentation-plan.md        # Presentation outline
├── package.json                # Dependencies (mermaid, reveal.js, etc)
├── vite.config.js             # Build configuration
└── build-inline.js            # Standalone build script
```

## Presentation Structure

The presentation covers:
1. **The Challenge** - High-frequency telemetry from video games
2. **The Problem** - Traditional tools force compromises
3. **Our Approach** - Make recording data cheap through tail sampling
4. **Architecture** - Mermaid diagrams showing ingestion and analytics flows
5. **Stage 1: Instrumentation** - With nested Rust and Unreal code examples
6. **Stages 2-4** - Ingestion, Analytics, User Interfaces
7. **Cost Analysis** - Real production numbers ($1,000/month for 449B events)

### Navigation

- **Right/Left**: Navigate between main sections
- **Down/Up**: Vertical slides (e.g., code examples under Stage 1)
- Press `Esc` for overview mode

### Mermaid Diagrams

Architecture flows use Mermaid with dark theme styling:
```markdown
\`\`\`mermaid
graph LR
    Apps[Client] --> Server
    style Apps fill:#2d2d2d,stroke:#4a9eff,color:#e6e6e6
\`\`\`
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

1. **Static Hosting**: Deploy `dist/` folder to any web server
2. **Standalone File**: Share `dist/presentation-inline.html` directly (works offline)
3. **GitHub Pages**: Push to gh-pages branch for easy hosting

## Speaker Notes

See `presentation-plan.md` for detailed speaker notes and timing for each section.

## References

- Micromegas: https://github.com/madesroches/micromegas
- Reveal.js: https://revealjs.com
- Mermaid: https://mermaid.js.org