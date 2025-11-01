<!-- .slide: class="title-slide center" -->
# Advanced REST API Design
## Building Scalable Microservices

<div class="metadata">
<span class="version">2.1.0</span> | <span class="commit">a1b2c3d</span><br>
Technical Architecture Summit 2024
</div>

Note: This presentation covers advanced REST API design patterns, focusing on microservices architecture, performance optimization, and developer experience.

---

<!-- .slide: class="architecture center" -->
## System Architecture Overview

<div class="diagram-container">
<div class="diagram-title">Microservices Architecture</div>

<div class="component">API Gateway</div>
<span class="arrow">→</span>
<div class="component">Auth Service</div>
<span class="arrow">→</span>
<div class="component">User Service</div>

<br><br>

<div class="component">Load Balancer</div>
<span class="arrow">→</span>
<div class="component">Service Mesh</div>
<span class="arrow">→</span>
<div class="component">Database</div>

</div>

---

<!-- .slide: class="code-walkthrough" -->
## Authentication Middleware Implementation

<div class="code-panel">

```javascript
// Enhanced JWT middleware with caching
class AuthMiddleware {
  constructor(options = {}) {
    this.cache = new Map();
    this.cacheTimeout = options.cacheTimeout || 300000; // 5 min
    this.secretKey = process.env.JWT_SECRET;
  }

  async authenticate(req, res, next) {
    const token = this.extractToken(req);
    
    if (!token) {
      return res.status(401).json({ error: 'No token provided' });
    }

    // Check cache first
    const cached = this.cache.get(token);
    if (cached && Date.now() - cached.timestamp < this.cacheTimeout) {
      req.user = cached.user;
      return next();
    }

    try {
      const decoded = jwt.verify(token, this.secretKey);
      const user = await this.validateUser(decoded.userId);
      
      // Cache the result
      this.cache.set(token, {
        user,
        timestamp: Date.now()
      });
      
      req.user = user;
      next();
    } catch (error) {
      res.status(401).json({ error: 'Invalid token' });
    }
  }
}
```

</div>

<div class="explanation-panel">

### Key Features

- **Token Caching** <!-- .element: class="fragment" -->
  - Reduces database calls
  - Improves response times
  - Memory-efficient with TTL

- **Error Handling** <!-- .element: class="fragment" -->  
  - Structured error responses
  - Proper HTTP status codes
  - Security considerations

- **Async/Await** <!-- .element: class="fragment" -->
  - Clean code structure
  - Better error handling
  - Improved readability

</div>

Note: This middleware implementation demonstrates several best practices including caching, proper error handling, and secure token validation.

---

<!-- .slide: class="terminal-demo" -->
## Database Migration Process

<div class="terminal-window">
<div class="terminal-header">
<span class="terminal-dot red"></span>
<span class="terminal-dot yellow"></span>
<span class="terminal-dot green"></span>
<span class="terminal-title">migration-terminal</span>
</div>
<div class="terminal-body">

<div class="prompt">npm run migrate:status</div>
<div class="output">
┌─────────────────┬──────────┬─────────────────────┐
│ Migration       │ Status   │ Executed            │
├─────────────────┼──────────┼─────────────────────┤
│ 001_initial     │ <span class="success">✓ Done</span>    │ 2024-01-15 10:30   │
│ 002_add_users   │ <span class="success">✓ Done</span>    │ 2024-01-16 14:22   │
│ 003_add_indexes │ <span class="warning">⚠ Pending</span> │ -                   │
└─────────────────┴──────────┴─────────────────────┘
</div>

<div class="prompt">npm run migrate:up</div>
<div class="output success">✓ Migration 003_add_indexes applied successfully</div>
<div class="output">  - Created index on users.email</div>
<div class="output">  - Created index on posts.user_id</div>
<div class="output">  - Query performance improved by 85%</div>

<div class="prompt">npm run test:integration</div>
<div class="output success">✓ All 47 integration tests passed</div>

</div>
</div>

---

<!-- .slide: class="split-comparison" -->
## Performance Comparison

<div class="left-panel">

### Before Optimization

```javascript
// Naive implementation
async function getUserPosts(userId) {
  const user = await User.findById(userId);
  const posts = await Post.find({ userId });
  
  for (let post of posts) {
    post.comments = await Comment.find({ 
      postId: post.id 
    });
  }
  
  return { user, posts };
}
```

**Performance:**
- 🐌 **Response Time**: 2.3s
- 🔥 **DB Queries**: 15+ queries
- 💾 **Memory Usage**: 45MB

</div>

<div class="vs-divider">VS</div>

<div class="right-panel">

### After Optimization

```javascript
// Optimized with joins and batching
async function getUserPosts(userId) {
  const [user, postsWithComments] = await Promise.all([
    User.findById(userId),
    Post.aggregate([
      { $match: { userId } },
      { $lookup: {
          from: 'comments',
          localField: '_id',
          foreignField: 'postId',
          as: 'comments'
        }
      }
    ])
  ]);
  
  return { user, posts: postsWithComments };
}
```

**Performance:**
- ⚡ **Response Time**: 180ms
- ✅ **DB Queries**: 2 queries
- 💚 **Memory Usage**: 8MB

</div>

Note: The optimization reduced response time by 92% and memory usage by 82% through database query optimization and parallel processing.

---

<!-- .slide: class="api-docs" -->
## API Endpoints Documentation

### User Management

<div class="endpoint">
<span class="method get">GET</span>
<span class="endpoint-url">/api/v2/users/{id}</span>

**Description:** Retrieve user profile with optional data inclusion

**Parameters:**
- `id` (path) - User unique identifier
- `include` (query) - Comma-separated list: `posts,followers,stats`

<div class="response-example">

```json
{
  "data": {
    "id": "user_123",
    "email": "john@example.com",
    "profile": {
      "name": "John Doe",
      "avatar": "https://cdn.example.com/avatars/123.jpg"
    },
    "stats": {
      "posts": 42,
      "followers": 1337,
      "following": 256
    }
  },
  "meta": {
    "cached": true,
    "response_time": "23ms"
  }
}
```

</div>
</div>

<div class="endpoint">
<span class="method post">POST</span>
<span class="endpoint-url">/api/v2/users/{id}/posts</span>

**Description:** Create a new post for the specified user

**Request Body:**

```json
{
  "title": "Building REST APIs",
  "content": "Best practices for scalable API design...",
  "tags": ["api", "backend", "nodejs"],
  "visibility": "public"
}
```

</div>

---

<!-- .slide: class="metrics" -->
## Performance Metrics Dashboard

<div class="metric-grid">

<div class="metric-card">
<div class="metric-value">99.9%</div>
<div class="metric-label">Uptime</div>
<div class="metric-trend trend-up">↗ +0.2% vs last month</div>
</div>

<div class="metric-card">
<div class="metric-value">147ms</div>
<div class="metric-label">Avg Response</div>
<div class="metric-trend trend-down">↘ -23ms improved</div>
</div>

<div class="metric-card">
<div class="metric-value">2.4K</div>
<div class="metric-label">Requests/sec</div>
<div class="metric-trend trend-up">↗ +15% vs last week</div>
</div>

<div class="metric-card">
<div class="metric-value">0.01%</div>
<div class="metric-label">Error Rate</div>
<div class="metric-trend trend-down">↘ -50% reduced</div>
</div>

<div class="metric-card">
<div class="metric-value">34GB</div>
<div class="metric-label">Data Processed</div>
<div class="metric-trend trend-up">↗ Daily average</div>
</div>

<div class="metric-card">
<div class="metric-value">12</div>
<div class="metric-label">Active Services</div>
<div class="metric-trend">→ Stable</div>
</div>

</div>

Note: These metrics show our system performance over the last 30 days, highlighting the improvements from our recent optimization efforts.

---

<!-- .slide: class="documentation" -->
## Quick Start Guide

<div class="doc-header">
<h3>Getting Started with the API</h3>
<p>Follow these steps to integrate with our REST API</p>
</div>

<div class="doc-section">
<h4>Installation</h4>

<span class="command">npm install @company/api-client</span>
<span class="command">yarn add @company/api-client</span>

</div>

<div class="doc-section">
<h4>Authentication</h4>

First, obtain your API key from the developer console:

```javascript
const client = new ApiClient({
  apiKey: process.env.API_KEY,
  baseUrl: 'https://api.company.com/v2'
});
```

</div>

<div class="doc-section">
<h4>Basic Usage</h4>

```javascript
// Fetch user data
const user = await client.users.get('123');

// Create a new post
const post = await client.posts.create({
  title: 'Hello World',
  content: 'My first API post!'
});

// Handle errors gracefully
try {
  const result = await client.users.update('123', userData);
} catch (error) {
  console.error('API Error:', error.message);
}
```

</div>

---

## Summary

### What We've Covered

- 🏗️ **Architecture patterns** for scalable microservices
- 🔐 **Authentication strategies** with JWT and caching
- ⚡ **Performance optimization** techniques
- 📊 **Monitoring and metrics** for production systems
- 📚 **Documentation standards** for developer experience

### Next Steps

1. **Implement rate limiting** for API protection
2. **Add GraphQL layer** for flexible data fetching  
3. **Set up distributed tracing** for better observability
4. **Create SDK packages** for multiple languages

---

<!-- .slide: class="center" -->
## Questions & Discussion

**API Documentation**: [docs.company.com/api](https://docs.company.com/api)  
**GitHub Repository**: [github.com/company/api](https://github.com/company/api)  
**Slack Channel**: #api-development

<div style="margin-top: 2em; font-family: var(--font-mono); color: var(--color-text-secondary);">
Press <kbd>S</kbd> for speaker notes | <kbd>ESC</kbd> for overview | <kbd>?</kbd> for help
</div>

Note: Thank you for attending this technical deep dive. The complete code examples and additional resources are available in our GitHub repository. Feel free to reach out on Slack for any follow-up questions or discussion about implementation details.