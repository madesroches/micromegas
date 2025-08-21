# Analytics Web Application

!!! warning "Early Development Stage"
    The Analytics Web Application is in a very early stage of development and is currently **only suitable for local testing and development**. It is not recommended for production use at this time.

The Analytics Web Application provides a modern web interface for exploring Micromegas telemetry data, generating Perfetto traces, and monitoring process activity.

## Overview

The analytics web app consists of:

- **Backend**: Rust-based web server using Axum framework
- **Frontend**: Next.js React application with TypeScript
- **Integration**: Direct FlightSQL connection to analytics service

## Features

### Process Explorer
- View all active processes with metadata
- Real-time process list updates
- Process filtering and sorting

### Log Viewer
- Stream log entries with level filtering (Fatal, Error, Warn, Info, Debug, Trace)
- Configurable entry limits (50/100/200/500)
- Color-coded log levels for easy identification

### Trace Generation
- Generate Perfetto traces from process data
- Real-time progress updates during generation
- HTTP streaming for large trace files
- Download traces in protobuf format

### Process Statistics
- View detailed process metrics
- Thread count and activity monitoring
- Log entry counts and trace event statistics

## Security Configuration

The analytics web server includes several security configuration options:

### CORS Configuration

Configure Cross-Origin Resource Sharing (CORS) using environment variables:

```bash
# Development mode - allows any origin (use "*")
export ANALYTICS_WEB_CORS_ORIGIN="*"

# Production mode - restrict to specific origin
export ANALYTICS_WEB_CORS_ORIGIN="https://your-domain.com"

# Local development (default)
export ANALYTICS_WEB_CORS_ORIGIN="http://localhost:3000"
```

**Security Notes:**
- **Development**: Use `"*"` only in development environments
- **Production**: Always specify exact origins in production
- **Default**: If not set, defaults to `http://localhost:3000`

### Authentication

The analytics web server uses FlightSQL authentication:

```bash
# Required for production deployments
export MICROMEGAS_AUTH_TOKEN="your-secure-token"
```

**Security Notes:**
- If `MICROMEGAS_AUTH_TOKEN` is not set, authentication uses empty string
- Always set a secure token in production environments
- Token is passed to FlightSQL client for all database queries

### Environment Variables

| Variable | Description | Default | Security Impact |
|----------|-------------|---------|-----------------|
| `ANALYTICS_WEB_CORS_ORIGIN` | CORS allowed origin | `http://localhost:3000` | **High** - Controls cross-origin access |
| `MICROMEGAS_AUTH_TOKEN` | FlightSQL authentication token | `""` (empty) | **High** - Database access control |

## Deployment

### Development Setup

1. **Start Backend Server**:
   ```bash
   cd rust
   cargo run --bin analytics-web-srv -- --port 8000
   ```

2. **Start Frontend Development Server**:
   ```bash
   cd analytics-web-app
   npm run dev
   ```

3. **Environment Configuration**:
   ```bash
   # For development
   export ANALYTICS_WEB_CORS_ORIGIN="*"
   export MICROMEGAS_AUTH_TOKEN="your-dev-token"
   ```

### Production Deployment

!!! danger "Not Ready for Production"
    The analytics web application is not yet ready for production deployment. Use only for local development and testing.

For future production deployment considerations:

1. **Build Frontend**:
   ```bash
   cd analytics-web-app
   npm run build
   ```

2. **Configure Security**:
   ```bash
   # Restrict CORS to your domain
   export ANALYTICS_WEB_CORS_ORIGIN="https://analytics.yourdomain.com"
   
   # Set secure authentication token
   export MICROMEGAS_AUTH_TOKEN="$(openssl rand -hex 32)"
   ```

3. **Start Production Server**:
   ```bash
   cd rust
   cargo run --release --bin analytics-web-srv -- --port 8000 --frontend-dir ../analytics-web-app/dist
   ```

## Security Best Practices

### 🔴 Critical Security Measures

1. **CORS Configuration**
   - Never use `"*"` in production
   - Specify exact origins that need access
   - Regularly review and update allowed origins

2. **Authentication**
   - Always set `MICROMEGAS_AUTH_TOKEN` in production
   - Use strong, randomly generated tokens
   - Rotate tokens regularly

3. **Network Security**
   - Deploy behind reverse proxy (nginx, Apache)
   - Use HTTPS in production
   - Implement rate limiting

### 🟡 Additional Security Considerations

1. **Input Validation**
   - Process IDs are validated as UUIDs
   - SQL queries use DataFusion read-only context
   - All user inputs are properly sanitized

2. **Error Handling**
   - Structured error responses without sensitive data
   - Proper logging of security events
   - Graceful handling of authentication failures

3. **Dependencies**
   - Regular security audits with `cargo audit`
   - Keep dependencies updated
   - Monitor for vulnerability advisories

## Monitoring and Observability

The analytics web server includes built-in observability:

- **Health Checks**: `/analyticsweb/health` endpoint
- **Request Tracing**: All requests traced with correlation IDs
- **Error Logging**: Structured error logging with context
- **Metrics**: Performance metrics for all endpoints

## API Reference

### Health Check
```
GET /analyticsweb/health
```
Returns service health and FlightSQL connection status.

### Process Management
```
GET /analyticsweb/processes
GET /analyticsweb/process/{id}/statistics
GET /analyticsweb/process/{id}/log-entries?level={level}&limit={limit}
```

### Trace Generation
```
GET /analyticsweb/perfetto/{id}/info
POST /analyticsweb/perfetto/{id}/generate
POST /analyticsweb/perfetto/{id}/validate
```

All API endpoints return structured JSON responses with proper error handling.

## Troubleshooting

### Common Issues

1. **CORS Errors**
   - Check `ANALYTICS_WEB_CORS_ORIGIN` setting
   - Verify frontend and backend URLs match
   - Clear browser cache

2. **Authentication Failures**
   - Verify `MICROMEGAS_AUTH_TOKEN` is set
   - Check FlightSQL server connectivity
   - Review authentication logs

3. **Trace Generation Errors**
   - Check FlightSQL connection health
   - Verify process ID exists in database
   - Monitor server logs for errors

### Log Analysis

Check server logs for detailed error information:

```bash
# View real-time logs
tail -f /tmp/analytics.log

# Search for errors
grep "ERROR" /tmp/analytics.log
```

## Contributing

To contribute to the analytics web app:

1. Follow the [Contributing Guide](contributing.md)
2. Review security guidelines before submitting changes
3. Include tests for new security features
4. Update documentation for configuration changes