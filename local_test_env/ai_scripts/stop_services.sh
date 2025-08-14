#!/bin/bash

# Simple script to stop micromegas services
# Usage: ./stop_services.sh

echo "🛑 Stopping micromegas services..."

if [ -f /tmp/micromegas_pids.txt ]; then
    PIDS=$(cat /tmp/micromegas_pids.txt)
    echo "Killing PIDs: $PIDS"
    kill $PIDS 2>/dev/null || true
    rm -f /tmp/micromegas_pids.txt
fi

# Kill any remaining services
pkill -f "telemetry-ingestion-srv" || true
pkill -f "flight-sql-srv" || true  
pkill -f "telemetry-admin" || true

echo "✅ Services stopped"

# Clean up log files
rm -f /tmp/ingestion.log /tmp/analytics.log /tmp/admin.log