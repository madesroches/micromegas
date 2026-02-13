# Changelog

## 0.20.0 (2026-02-12)

Version sync release - no plugin-specific changes.

## 0.19.0 (2026-01-28)

Version sync release - no plugin-specific changes.

## 0.18.0 (2026-01-08)

Version sync release - no plugin-specific changes.

## 0.17.0 (2025-12-12)

### Improvements
- Update plugin logo to Micromegas branding (#617)

### Bug Fixes
- Fix UTF-8 user attribution headers with percent-encoding (#638)

## 0.16.0 (2025-11-28)

### Bug Fixes
- Fix Grafana plugin packaging and document release process (#601)
- Fix secureJsonData undefined error and rename plugin to Micromegas FlightSQL (#603)

## 0.15.0 (2025-11-14)

First release from the main Micromegas repository (previously released separately).

### Features

#### Core Functionality
- **FlightSQL Datasource Integration**: Native Apache Arrow FlightSQL protocol support for querying Micromegas telemetry data
- **SQL Query Editor**: Full-featured SQL editor with syntax highlighting and formatting
- **Query Builder**: Visual query builder for constructing queries without writing SQL
- **Query Variables**: Support for Grafana dashboard template variables with dynamic value population

#### Authentication
- **OAuth 2.0 / OIDC Authentication**: Secure authentication using OpenID Connect providers
- **API Key Authentication**: Alternative authentication method using API keys
- **Token Management**: Automatic token refresh and secure credential storage

#### Developer Experience
- **TypeScript Codebase**: Fully typed implementation for better maintainability
- **Comprehensive Test Suite**: Unit and integration tests for reliability
- **CI/CD Pipeline**: Automated build, test, and release workflow
- **Migration Tools**: Utilities for migrating datasource configurations

#### Documentation
- **Installation Guide**: Complete installation instructions for various deployment methods
- **Configuration Guide**: Detailed datasource configuration documentation
- **Authentication Guide**: Step-by-step OAuth 2.0 and API key setup
- **Usage Guide**: Query examples and best practices
- **Troubleshooting Guide**: Common issues and solutions

### Technical Details
- Based on Grafana plugin SDK 11.6.7
- Compatible with Grafana 9.0+
- Uses Apache Arrow FlightSQL for efficient data transfer
- Supports both gRPC and gRPC-Web protocols

### Security
- Fixed 28 Dependabot security vulnerabilities
- Updated to latest Grafana plugin SDK for security patches
- Secure credential handling with OAuth 2.0 flow