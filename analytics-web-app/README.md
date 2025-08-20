# Analytics Web App

Modern web application for exploring and analyzing micromegas telemetry data with advanced querying and export capabilities.

## Features

- 🔍 Process discovery and filtering
- 📊 Interactive telemetry data exploration
- 📥 Multiple export formats (Perfetto traces, CSV, JSON, Parquet)
- 📈 Real-time data visualization and analytics
- 🔄 HTTP streaming for efficient large data transfers
- 🎨 Modern React UI with Tailwind CSS
- 🔄 Real-time health monitoring
- 📱 Responsive design

## Architecture

- **Backend**: Rust + Axum web server (`analytics-web-srv`)
- **Frontend**: Next.js 15 + React 18 + TypeScript
- **UI**: Tailwind CSS + Radix UI components
- **API**: REST endpoints with HTTP streaming support

## Development

### Prerequisites

- Node.js 18+ 
- Rust 1.70+
- FlightSQL server running on port 50051

### Frontend Development

```bash
cd analytics-web-app
npm install
npm run dev
```

The frontend will run on http://localhost:3001 with hot reloading.

### Backend Development  

```bash
cd rust
cargo run --bin analytics-web-srv
```

The backend will run on http://localhost:3000.

### Full Stack Development

1. Start the backend server:
   ```bash
   cd rust && cargo run --bin analytics-web-srv
   ```

2. In another terminal, start the frontend:
   ```bash  
   cd analytics-web-app && npm run dev
   ```

The Next.js dev server will proxy API requests to the Rust backend.

## Production Build

1. Build the frontend:
   ```bash
   cd analytics-web-app
   npm run build
   ```

2. Run the backend with static file serving:
   ```bash
   cd rust
   cargo run --bin analytics-web-srv -- --frontend-dir ../analytics-web-app/dist
   ```

The entire application will be served from http://localhost:3000.

### Quick Start

To start both frontend and backend in development mode:

```bash
cd analytics-web-app
./start_analytics_web.py
```

This will automatically start both the Rust backend and Next.js frontend with hot reloading.

## API Endpoints

- `GET /api/health` - Health check and system status
- `GET /api/processes` - List available processes  
- `GET /api/perfetto/{process_id}/info` - Get Perfetto trace metadata
- `POST /api/perfetto/{process_id}/generate` - Generate Perfetto trace with streaming progress
- `GET /api/data/{process_id}/query` - Query telemetry data with SQL
- `POST /api/data/{process_id}/export` - Export data in various formats (CSV, JSON, Parquet)

## Environment Variables

- `MICROMEGAS_FLIGHTSQL_URL` - FlightSQL server address (default: grpc://127.0.0.1:50051)
- `MICROMEGAS_AUTH_TOKEN` - Authentication token for FlightSQL server
- `PORT` - Server port (default: 3000)
- `FRONTEND_DIR` - Frontend build directory (default: ../analytics-web-app/dist)