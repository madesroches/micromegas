import Reveal from 'reveal.js';
import Markdown from 'reveal.js/plugin/markdown/markdown.esm.js';
import Highlight from 'reveal.js/plugin/highlight/highlight.esm.js';
import Notes from 'reveal.js/plugin/notes/notes.esm.js';
import Search from 'reveal.js/plugin/search/search.esm.js';
import Zoom from 'reveal.js/plugin/zoom/zoom.esm.js';
import RevealMermaid from 'reveal.js-mermaid-plugin';

// Initialize Reveal.js with basic configuration
let deck = new Reveal({
    // Core settings
    hash: true,
    controls: true,
    progress: true,
    center: true,
    touch: true,
    transition: 'slide',

    // Show slide numbers
    slideNumber: 'c/t',

    // Plugins
    plugins: [
        Markdown,
        Highlight,
        Notes,
        Search,
        Zoom,
        RevealMermaid
    ]
});

// Initialize
deck.initialize();
