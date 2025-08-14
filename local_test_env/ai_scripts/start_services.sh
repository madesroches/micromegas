#!/bin/bash

# Simple script to start micromegas services for testing
# Usage: ./start_services.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUST_DIR="$SCRIPT_DIR/rust"

echo "🔧 Building services..."
cd "$RUST_DIR"
cargo build

echo "🚀 Starting services..."

# Kill any existing services
pkill -f "telemetry-ingestion-srv" || true
pkill -f "flight-sql-srv" || true
pkill -f "telemetry-admin" || true
sleep 2

# Start PostgreSQL in background (if not already running)
echo "🐘 Checking PostgreSQL..."
if ! pgrep -f "postgres" > /dev/null; then
    echo "Starting PostgreSQL..."
    cd "$SCRIPT_DIR/local_test_env/db"
    python3 run.py &
    POSTGRES_PID=$!
    echo "PostgreSQL PID: $POSTGRES_PID"
    sleep 5
else
    echo "PostgreSQL already running"
fi

cd "$RUST_DIR"

# Start Ingestion Server
echo "📥 Starting Ingestion Server..."
cargo run -p telemetry-ingestion-srv -- --listen-endpoint-http 127.0.0.1:9000 > /tmp/ingestion.log 2>&1 &
INGESTION_PID=$!
echo "Ingestion Server PID: $INGESTION_PID"

# Wait for ingestion server to be ready
echo "⏳ Waiting for Ingestion Server..."
for i in {1..30}; do
    if curl -s http://127.0.0.1:9000/health > /dev/null 2>&1; then
        echo "✅ Ingestion Server is ready!"
        break
    fi
    if [ $i -eq 30 ]; then
        echo "❌ Ingestion Server failed to start"
        exit 1
    fi
    sleep 1
done

# Start Analytics Server
echo "📊 Starting Analytics Server..."
cargo run -p flight-sql-srv -- --disable-auth > /tmp/analytics.log 2>&1 &
ANALYTICS_PID=$!
echo "Analytics Server PID: $ANALYTICS_PID"

# Start Admin Daemon
echo "⚙️ Starting Admin Daemon..."
cargo run -p telemetry-admin -- crond > /tmp/admin.log 2>&1 &
ADMIN_PID=$!
echo "Admin Daemon PID: $ADMIN_PID"

echo ""
echo "🎉 All services started!"
echo "📥 Ingestion Server: http://127.0.0.1:9000"
echo "📊 Analytics Server: port 32010"
echo ""
echo "PIDs:"
echo "  Ingestion: $INGESTION_PID"
echo "  Analytics: $ANALYTICS_PID" 
echo "  Admin: $ADMIN_PID"
echo ""
echo "Logs:"
echo "  tail -f /tmp/ingestion.log"
echo "  tail -f /tmp/analytics.log"
echo "  tail -f /tmp/admin.log"
echo ""
echo "To stop services: kill $INGESTION_PID $ANALYTICS_PID $ADMIN_PID"

# Save PIDs for cleanup script
echo "$INGESTION_PID $ANALYTICS_PID $ADMIN_PID" > /tmp/micromegas_pids.txt

echo ""
echo "⏳ Waiting a moment for services to fully start..."
sleep 3

echo "✅ Ready to test!"