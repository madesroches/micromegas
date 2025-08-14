# Micromegas Documentation Migration to MkDocs

This document outlines the migration of Micromegas documentation from the current format to MkDocs.

## What Was Done

### 1. MkDocs Setup
- Created `mkdocs.yml` configuration with Material theme
- Set up documentation structure in `docs/` directory
- Configured navigation, themes, and extensions

### 2. Documentation Structure
```
docs/
├── index.md                    # Main landing page
├── getting-started.md          # Installation and setup guide
├── query-guide/               # SQL query documentation
│   ├── index.md               # Query guide overview
│   ├── quick-start.md         # Basic queries and examples
│   ├── python-api.md          # Complete Python API reference
│   ├── schema-reference.md    # Views, fields, and data types
│   ├── functions-reference.md # SQL functions
│   ├── query-patterns.md      # Common patterns
│   ├── performance.md         # Performance optimization
│   └── advanced-features.md   # View materialization, etc.
├── architecture/              # System architecture
│   └── index.md
└── development/               # Development guides
```

### 3. Content Migration
- **Migrated from**: `doc/how_to_query/README.md` (1233 lines)
- **Migrated to**: Multiple focused pages in `docs/query-guide/`
- **Improvements**:
  - Better organization with focused pages
  - Enhanced navigation and cross-linking
  - Material Design theme for better readability
  - Responsive design for mobile/tablet viewing

### 4. Key Features Added
- **Search functionality** across all documentation
- **Code syntax highlighting** for SQL and Python
- **Cross-references** between related sections
- **Mobile-responsive design**
- **Dark/light theme toggle**
- **Navigation breadcrumbs**

## How to Use

### Development Server
```bash
# Install dependencies
pip install -r docs-requirements.txt

# Start development server
mkdocs serve

# Visit http://localhost:8000
```

### Build Static Site
```bash
# Build documentation
mkdocs build

# Output in site/ directory
```

### Automated Build
```bash
# Use the build script
python build-docs.py
```

## Content Status

### ✅ Completed
- [x] Main landing page with project overview
- [x] Getting started guide adapted from existing docs
- [x] Query guide overview and navigation
- [x] Quick start with essential examples
- [x] Complete Python API reference
- [x] Comprehensive schema reference
- [x] Functions reference with examples
- [x] MkDocs configuration and theming

### 🚧 In Progress
- [ ] Complete query patterns section
- [ ] Full performance optimization guide
- [ ] Advanced features documentation
- [ ] Architecture deep dive
- [ ] Development guides

### 📋 Benefits Over Previous Format

1. **Better Organization**: Single 1233-line file split into focused, manageable pages
2. **Enhanced Navigation**: Sidebar navigation with sections and subsections
3. **Improved Readability**: Material Design theme with proper typography
4. **Better Mobile Experience**: Responsive design works on all devices
5. **Search Functionality**: Full-text search across all documentation
6. **Cross-linking**: Easy navigation between related topics
7. **Code Highlighting**: Better syntax highlighting for SQL and Python
8. **Maintainability**: Easier to update and maintain individual sections

## Next Steps

1. **Complete remaining sections**: Finish query patterns, performance guide, and architecture docs
2. **Review and refine**: Review all content for accuracy and completeness  
3. **Add examples**: More practical examples in query patterns
4. **Deploy**: Set up automated deployment (GitHub Pages, Netlify, etc.)
5. **Integrate with CI**: Add docs building to CI pipeline

## Configuration Details

### MkDocs Configuration (`mkdocs.yml`)
- **Theme**: Material Design with blue color scheme
- **Features**: Navigation tabs, search, code copying, TOC following
- **Extensions**: Admonitions, code highlighting, tables, cross-references
- **Plugins**: Search, mkdocstrings for API docs

### Dependencies (`docs-requirements.txt`)
- `mkdocs>=1.5.0` - Core MkDocs
- `mkdocs-material>=9.0.0` - Material Design theme
- `mkdocstrings>=0.22.0` - API documentation generation

The migration successfully transforms a large, monolithic documentation file into a modern, navigable, and maintainable documentation site that will be much easier for users to discover and consume the information they need.
