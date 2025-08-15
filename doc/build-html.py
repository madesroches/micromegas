#!/usr/bin/env python3
"""
Build HTML documentation from Markdown sources.

This script converts the SQL documentation from Markdown to HTML with 
proper navigation and styling similar to DataFusion's documentation.
"""

import markdown
import re
import os
from pathlib import Path

def extract_table_of_contents(md_content):
    """Extract table of contents from markdown content."""
    toc_entries = []
    lines = md_content.split('\n')
    
    for line in lines:
        if line.startswith('#'):
            level = len(line) - len(line.lstrip('#'))
            if level <= 3:  # Only include h1, h2, h3 in navigation
                title = line.strip('#').strip()
                # Create anchor from title
                anchor = re.sub(r'[^\w\s-]', '', title.lower())
                anchor = re.sub(r'[-\s]+', '-', anchor)
                toc_entries.append({
                    'level': level,
                    'title': title,
                    'anchor': anchor
                })
    
    return toc_entries

def generate_navigation_html(toc_entries):
    """Generate left sidebar navigation HTML."""
    nav_html = '''
    <nav class="sidebar">
        <div class="sidebar-header">
            <h3><a href="/">Micromegas SQL</a></h3>
        </div>
        <div class="sidebar-content">
    '''
    
    current_level = 0
    for entry in toc_entries:
        level = entry['level']
        title = entry['title']
        anchor = entry['anchor']
        
        if level == 1:
            nav_html += f'            <div class="nav-section">\n'
            nav_html += f'                <a href="#{anchor}" class="nav-h1">{title}</a>\n'
        elif level == 2:
            nav_html += f'                <a href="#{anchor}" class="nav-h2">{title}</a>\n'
        elif level == 3:
            nav_html += f'                <a href="#{anchor}" class="nav-h3">{title}</a>\n'
    
    nav_html += '''
        </div>
    </nav>
    '''
    
    return nav_html

def build_html_documentation():
    """Build HTML documentation from markdown sources."""
    
    # Read the markdown file
    doc_dir = Path(__file__).parent
    md_file = doc_dir / 'how_to_query' / 'README.md'
    
    if not md_file.exists():
        print(f"❌ Markdown file not found: {md_file}")
        return False
    
    with open(md_file, 'r', encoding='utf-8') as f:
        md_content = f.read()
    
    # Extract table of contents for navigation
    toc_entries = extract_table_of_contents(md_content)
    navigation_html = generate_navigation_html(toc_entries)
    
    # Configure markdown with extensions for better rendering
    md = markdown.Markdown(extensions=['tables', 'fenced_code', 'toc', 'attr_list'])
    html_content = md.convert(md_content)
    
    # Create full HTML page with navigation
    full_html = f'''<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Micromegas SQL Documentation</title>
    <style>
        :root {{
            --sidebar-width: 300px;
            --primary-color: #0969da;
            --border-color: #d0d7de;
            --bg-secondary: #f6f8fa;
            --text-primary: #24292e;
            --text-secondary: #656d76;
        }}
        
        * {{
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }}
        
        body {{ 
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            line-height: 1.6;
            color: var(--text-primary);
            background: white;
        }}
        
        .container {{
            display: flex;
            min-height: 100vh;
        }}
        
        /* Sidebar Navigation */
        .sidebar {{
            width: var(--sidebar-width);
            background: var(--bg-secondary);
            border-right: 1px solid var(--border-color);
            position: fixed;
            height: 100vh;
            overflow-y: auto;
            z-index: 100;
        }}
        
        .sidebar-header {{
            padding: 20px;
            border-bottom: 1px solid var(--border-color);
            background: white;
        }}
        
        .sidebar-header h3 {{
            font-size: 18px;
            font-weight: 600;
        }}
        
        .sidebar-header a {{
            color: var(--text-primary);
            text-decoration: none;
        }}
        
        .sidebar-content {{
            padding: 20px 0;
        }}
        
        .nav-section {{
            margin-bottom: 16px;
        }}
        
        .sidebar a {{
            display: block;
            padding: 4px 20px;
            color: var(--text-secondary);
            text-decoration: none;
            border-left: 3px solid transparent;
            transition: all 0.2s ease;
        }}
        
        .sidebar a:hover {{
            color: var(--primary-color);
            background: rgba(9, 105, 218, 0.1);
        }}
        
        .nav-h1 {{
            font-weight: 600;
            color: var(--text-primary) !important;
            margin-top: 12px;
            font-size: 14px;
        }}
        
        .nav-h2 {{
            padding-left: 32px !important;
            font-size: 13px;
        }}
        
        .nav-h3 {{
            padding-left: 44px !important;
            font-size: 12px;
            color: var(--text-secondary);
        }}
        
        /* Main Content */
        .main-content {{
            flex: 1;
            margin-left: var(--sidebar-width);
            padding: 40px;
            max-width: calc(100vw - var(--sidebar-width));
        }}
        
        .content {{
            max-width: 900px;
        }}
        
        /* Typography */
        h1, h2, h3, h4, h5, h6 {{ 
            color: var(--text-primary); 
            margin-top: 32px;
            margin-bottom: 16px;
            font-weight: 600;
            line-height: 1.25;
        }}
        
        h1 {{ 
            border-bottom: 1px solid var(--border-color); 
            padding-bottom: 10px;
            font-size: 32px;
            margin-top: 0;
        }}
        
        h2 {{ 
            border-bottom: 1px solid var(--border-color); 
            padding-bottom: 8px;
            font-size: 24px;
        }}
        
        h3 {{ font-size: 20px; }}
        h4 {{ font-size: 16px; }}
        
        p {{
            margin-bottom: 16px;
        }}
        
        /* Code styling */
        code {{ 
            background: var(--bg-secondary);
            padding: 2px 6px;
            border-radius: 3px;
            font-family: 'SFMono-Regular', Consolas, 'Liberation Mono', Menlo, monospace;
            font-size: 85%;
        }}
        
        pre {{ 
            background: var(--bg-secondary);
            padding: 16px;
            border-radius: 6px;
            overflow-x: auto;
            border: 1px solid var(--border-color);
            margin: 16px 0;
        }}
        
        pre code {{
            background: transparent;
            padding: 0;
            font-size: 14px;
        }}
        
        /* Tables */
        table {{ 
            border-collapse: collapse; 
            width: 100%; 
            margin: 24px 0;
            font-size: 14px;
        }}
        
        th, td {{ 
            border: 1px solid var(--border-color); 
            padding: 8px 12px; 
            text-align: left; 
        }}
        
        th {{ 
            background: var(--bg-secondary); 
            font-weight: 600;
        }}
        
        /* Links */
        a {{ 
            color: var(--primary-color); 
            text-decoration: none; 
        }}
        
        a:hover {{ text-decoration: underline; }}
        
        /* Lists */
        ul, ol {{ 
            padding-left: 32px; 
            margin-bottom: 16px;
        }}
        
        li {{ margin: 4px 0; }}
        
        /* Blockquotes */
        blockquote {{
            padding: 16px;
            margin: 16px 0;
            color: var(--text-secondary);
            border-left: 4px solid var(--border-color);
            background: var(--bg-secondary);
            border-radius: 0 6px 6px 0;
        }}
        
        /* Responsive design */
        @media (max-width: 1024px) {{
            :root {{
                --sidebar-width: 280px;
            }}
        }}
        
        @media (max-width: 768px) {{
            .sidebar {{
                transform: translateX(-100%);
                transition: transform 0.3s ease;
            }}
            
            .main-content {{
                margin-left: 0;
                max-width: 100vw;
                padding: 20px;
            }}
        }}
        
        /* Smooth scrolling for anchor links */
        html {{
            scroll-behavior: smooth;
        }}
        
        /* Highlight target sections */
        :target {{
            scroll-margin-top: 20px;
        }}
    </style>
</head>
<body>
    <div class="container">
        {navigation_html}
        
        <main class="main-content">
            <div class="content">
                {html_content}
            </div>
        </main>
    </div>
    
    <script>
        // Highlight current section in navigation
        document.addEventListener('DOMContentLoaded', function() {{
            const navLinks = document.querySelectorAll('.sidebar a[href^="#"]');
            const sections = document.querySelectorAll('h1, h2, h3');
            
            function highlightNav() {{
                let current = '';
                sections.forEach(section => {{
                    const sectionTop = section.offsetTop;
                    const sectionHeight = section.offsetHeight;
                    if (scrollY >= sectionTop - 100) {{
                        current = section.id;
                    }}
                }});
                
                navLinks.forEach(link => {{
                    link.style.borderLeftColor = 'transparent';
                    link.style.background = 'transparent';
                    if (link.getAttribute('href') === '#' + current) {{
                        link.style.borderLeftColor = 'var(--primary-color)';
                        link.style.background = 'rgba(9, 105, 218, 0.1)';
                    }}
                }});
            }}
            
            window.addEventListener('scroll', highlightNav);
            highlightNav(); // Initial highlight
        }});
    </script>
</body>
</html>'''
    
    # Create output directory
    output_dir = doc_dir / 'how_to_query' / 'html'
    output_dir.mkdir(exist_ok=True)
    
    # Write the HTML file
    output_file = output_dir / 'index.html'
    with open(output_file, 'w', encoding='utf-8') as f:
        f.write(full_html)
    
    print(f"✅ Successfully generated HTML documentation: {output_file}")
    return True

if __name__ == "__main__":
    build_html_documentation()