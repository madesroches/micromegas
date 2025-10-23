#!/bin/bash
# Generate SVG flame graph from async events data
# Requires flamegraph.pl: https://github.com/brendangregg/FlameGraph

set -e

PROCESS_ID=${1:-""}
FLAME_DIR="flame_graphs"

if [ -z "$PROCESS_ID" ]; then
    echo "Usage: $0 <process_id>"
    echo "Example: $0 1333745d-77e3-4399-b937-c2562d9f526f"
    exit 1
fi

echo "🔥 Generating flame graph data for process: $PROCESS_ID"

# Generate flame graph data
cd ../../python/micromegas
poetry run python ../../tasks/perf_report/async_events_analyzer.py "$PROCESS_ID" --flame-only

# Find the most recent .txt file
FLAME_FILE=$(ls -t $FLAME_DIR/async_events_*.txt | head -1)

if [ ! -f "$FLAME_FILE" ]; then
    echo "❌ No flame graph data file found"
    exit 1
fi

echo "📊 Found flame data: $FLAME_FILE"

# Check if flamegraph.pl is available
if command -v flamegraph.pl >/dev/null 2>&1; then
    echo "🎨 Generating SVG flame graph..."
    OUTPUT_SVG="${FLAME_FILE%.txt}.svg"
    cat "$FLAME_FILE" | flamegraph.pl --title "Async Events Performance" > "$OUTPUT_SVG"
    echo "✅ Generated: $OUTPUT_SVG"
    
    # Try to open in browser (Linux/WSL)
    if command -v xdg-open >/dev/null 2>&1; then
        echo "🌐 Opening in browser..."
        xdg-open "$OUTPUT_SVG"
    elif command -v wslview >/dev/null 2>&1; then
        echo "🌐 Opening in Windows browser..."
        wslview "$OUTPUT_SVG"
    fi
else
    echo "⚠️  flamegraph.pl not found"
    echo "Install from: https://github.com/brendangregg/FlameGraph"
    echo "Then run: cat $FLAME_FILE | flamegraph.pl > flame.svg"
fi

# Show HTML alternative
HTML_FILE="${FLAME_FILE%.txt}.html"
if [ -f "$HTML_FILE" ]; then
    echo "💡 Alternative: Open $HTML_FILE in your browser for immediate visualization"
fi