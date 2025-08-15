#!/usr/bin/env python3
"""
Async Events Performance Analyzer

This script performs comprehensive performance analysis of async events data
from Micromegas, focusing on span names and operations rather than just modules.

Usage:
    python async_events_analyzer.py <process_id>

Example:
    python async_events_analyzer.py 1333745d-77e3-4399-b937-c2562d9f526f

Requirements:
    - Micromegas services must be running
    - Poetry environment with micromegas package installed
"""

import sys
import argparse
import pandas as pd
from datetime import datetime
from typing import Dict, Any, List, Tuple
import json
import os
from pathlib import Path
import micromegas


class AsyncEventsAnalyzer:
    """Analyzes async events performance data from Micromegas."""
    
    def __init__(self, process_id: str):
        self.process_id = process_id
        self.client = micromegas.connect()
        self.results = {}
    
    def run_analysis(self) -> Dict[str, Any]:
        """Run complete performance analysis and return results."""
        print(f"🔍 Analyzing async events for process: {self.process_id}")
        print("=" * 60)
        
        # 1. Dataset Overview
        print("📊 Dataset Overview...")
        self.results['overview'] = self._analyze_overview()
        self._print_overview()
        
        # 2. Span Names Distribution
        print("\n📋 Span Names Distribution...")
        self.results['span_names'] = self._analyze_span_names()
        self._print_span_names()
        
        # 3. Performance by Span Name
        print("\n⚡ Performance by Span Name...")
        self.results['performance'] = self._analyze_performance()
        self._print_performance()
        
        # 4. Slowest Individual Spans
        print("\n🐌 Slowest Individual Spans...")
        self.results['slowest_spans'] = self._analyze_slowest_spans()
        self._print_slowest_spans()
        
        # 5. Concurrency Analysis
        print("\n🚦 Concurrency Analysis...")
        self.results['concurrency'] = self._analyze_concurrency()
        self._print_concurrency()
        
        # 6. Generate Flame Graph Data (if requested)
        if not getattr(self, 'skip_flame_graphs', False):
            print("\n🔥 Generating Flame Graph Data...")
            self.results['flame_data'] = self._generate_flame_data()
            flame_files = self._create_flame_graphs()
            
            if flame_files:
                print(f"  Generated flame graphs:")
                for file_path in flame_files:
                    print(f"    📊 {file_path}")
        else:
            print("\n🔥 Skipping flame graph generation (--no-flame-graphs)")
            self.results['flame_data'] = pd.DataFrame()
        
        return self.results
    
    def _analyze_overview(self) -> pd.DataFrame:
        """Analyze dataset overview statistics."""
        sql = f"""
        SELECT COUNT(*) as total_events,
               COUNT(DISTINCT span_id) as unique_spans,
               COUNT(DISTINCT parent_span_id) as unique_parent_spans,
               MIN(time) as earliest_time,
               MAX(time) as latest_time,
               COUNT(DISTINCT event_type) as event_types,
               COUNT(DISTINCT target) as unique_targets,
               COUNT(DISTINCT filename) as unique_files
        FROM view_instance('async_events', '{self.process_id}')
        """
        return self.client.query(sql)
    
    def _analyze_span_names(self) -> pd.DataFrame:
        """Analyze distribution of span names and their event counts."""
        sql = f"""
        SELECT name,
               target,
               COUNT(*) as event_count,
               COUNT(DISTINCT span_id) as unique_spans,
               ROUND(COUNT(*) * 100.0 / SUM(COUNT(*)) OVER(), 2) as percentage
        FROM view_instance('async_events', '{self.process_id}')
        GROUP BY name, target
        ORDER BY event_count DESC
        """
        return self.client.query(sql)
    
    def _analyze_performance(self) -> pd.DataFrame:
        """Analyze performance metrics by span name."""
        sql = f"""
        WITH span_performance AS (
            SELECT 
                name,
                span_id,
                COUNT(*) as events_per_span,
                EXTRACT(EPOCH FROM (MAX(time) - MIN(time))) * 1000 as duration_ms
            FROM view_instance('async_events', '{self.process_id}')
            GROUP BY name, span_id
            HAVING COUNT(*) >= 2
        )
        SELECT 
            name,
            COUNT(*) as execution_count,
            ROUND(AVG(duration_ms), 2) as avg_duration_ms,
            ROUND(MIN(duration_ms), 2) as min_duration_ms,
            ROUND(MAX(duration_ms), 2) as max_duration_ms,
            ROUND(SUM(duration_ms), 2) as total_duration_ms,
            ROUND(STDDEV(duration_ms), 2) as stddev_ms
        FROM span_performance
        GROUP BY name
        ORDER BY total_duration_ms DESC
        """
        return self.client.query(sql)
    
    def _analyze_slowest_spans(self) -> pd.DataFrame:
        """Find the slowest individual span executions."""
        sql = f"""
        WITH span_performance AS (
            SELECT 
                name,
                span_id,
                COUNT(*) as events_per_span,
                MIN(time) as start_time,
                MAX(time) as end_time,
                EXTRACT(EPOCH FROM (MAX(time) - MIN(time))) * 1000 as duration_ms
            FROM view_instance('async_events', '{self.process_id}')
            GROUP BY name, span_id
            HAVING COUNT(*) >= 2
        )
        SELECT 
            name,
            span_id,
            ROUND(duration_ms, 2) as duration_ms,
            start_time,
            end_time
        FROM span_performance
        ORDER BY duration_ms DESC
        LIMIT 10
        """
        return self.client.query(sql)
    
    def _analyze_concurrency(self) -> pd.DataFrame:
        """Analyze concurrent execution patterns."""
        sql = f"""
        WITH concurrent_spans AS (
            SELECT 
                date_trunc('second', time) as time_window,
                COUNT(DISTINCT span_id) as active_spans
            FROM view_instance('async_events', '{self.process_id}')
            WHERE event_type = 'begin'
            GROUP BY date_trunc('second', time)
        )
        SELECT 
            MAX(active_spans) as max_concurrent_spans,
            ROUND(AVG(active_spans), 2) as avg_concurrent_spans,
            COUNT(*) as time_windows_analyzed
        FROM concurrent_spans
        """
        return self.client.query(sql)
    
    def _generate_flame_data(self) -> pd.DataFrame:
        """Generate flame graph data with span hierarchies and timing."""
        sql = f"""
        WITH span_details AS (
            SELECT 
                span_id,
                parent_span_id,
                name,
                target,
                MIN(time) as start_time,
                MAX(time) as end_time,
                EXTRACT(EPOCH FROM (MAX(time) - MIN(time))) * 1000000 as duration_us,
                COUNT(*) as event_count
            FROM view_instance('async_events', '{self.process_id}')
            GROUP BY span_id, parent_span_id, name, target
            HAVING COUNT(*) >= 2
        ),
        span_hierarchy AS (
            SELECT 
                s.*,
                CASE 
                    WHEN s.parent_span_id = 0 THEN s.name
                    ELSE CONCAT(p.name, ';', s.name)
                END as stack_trace
            FROM span_details s
            LEFT JOIN span_details p ON s.parent_span_id = p.span_id
        )
        SELECT 
            span_id,
            parent_span_id,
            name,
            target,
            stack_trace,
            duration_us,
            start_time,
            end_time
        FROM span_hierarchy
        ORDER BY start_time
        """
        return self.client.query(sql)
    
    def _create_flame_graphs(self) -> List[str]:
        """Create flame graph files in multiple formats."""
        flame_data = self.results['flame_data']
        if flame_data.empty:
            print("  No flame graph data available (no complete spans)")
            return []
        
        output_dir = Path("flame_graphs")
        output_dir.mkdir(exist_ok=True)
        
        # Generate timestamp for unique filenames
        timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
        process_short = self.process_id[:8]
        
        flame_files = []
        
        # 1. Generate Brendan Gregg format (for flamegraph.pl)
        gregg_file = output_dir / f"async_events_{process_short}_{timestamp}.txt"
        self._create_gregg_format(flame_data, gregg_file)
        flame_files.append(str(gregg_file))
        
        # 2. Generate JSON format (for d3-flame-graph)
        json_file = output_dir / f"async_events_{process_short}_{timestamp}.json"
        self._create_json_format(flame_data, json_file)
        flame_files.append(str(json_file))
        
        # 3. Generate simple HTML visualization
        html_file = output_dir / f"async_events_{process_short}_{timestamp}.html"
        self._create_html_visualization(flame_data, html_file)
        flame_files.append(str(html_file))
        
        return flame_files
    
    def _create_gregg_format(self, flame_data: pd.DataFrame, output_file: Path):
        """Create Brendan Gregg format for flamegraph.pl tool."""
        with open(output_file, 'w') as f:
            for _, row in flame_data.iterrows():
                # Extract operation name (last part after ::)
                operation = row['name'].split('::')[-1] if '::' in row['name'] else row['name']
                duration_us = int(row['duration_us'])
                
                # Create stack trace with module info
                module = row['target'].split('::')[-1] if '::' in row['target'] else row['target']
                stack = f"{module};{operation}"
                
                f.write(f"{stack} {duration_us}\n")
    
    def _create_json_format(self, flame_data: pd.DataFrame, output_file: Path):
        """Create JSON format for d3-flame-graph and similar tools."""
        # Group by operation for flame graph structure
        operations = {}
        
        for _, row in flame_data.iterrows():
            operation = row['name'].split('::')[-1] if '::' in row['name'] else row['name']
            module = row['target'].split('::')[-1] if '::' in row['target'] else row['target']
            duration_us = int(row['duration_us'])
            
            if module not in operations:
                operations[module] = {
                    "name": module,
                    "value": 0,
                    "children": {}
                }
            
            if operation not in operations[module]["children"]:
                operations[module]["children"][operation] = {
                    "name": operation,
                    "value": 0,
                    "children": []
                }
            
            operations[module]["value"] += duration_us
            operations[module]["children"][operation]["value"] += duration_us
        
        # Convert to d3 flame graph format
        flame_graph = {
            "name": "root",
            "children": []
        }
        
        for module_name, module_data in operations.items():
            module_node = {
                "name": module_name,
                "value": module_data["value"],
                "children": []
            }
            
            for op_name, op_data in module_data["children"].items():
                module_node["children"].append({
                    "name": op_name,
                    "value": op_data["value"]
                })
            
            flame_graph["children"].append(module_node)
        
        with open(output_file, 'w') as f:
            json.dump(flame_graph, f, indent=2)
    
    def _create_html_visualization(self, flame_data: pd.DataFrame, output_file: Path):
        """Create a simple HTML visualization of the flame graph."""
        # Calculate totals by operation
        operation_totals = flame_data.groupby('name')['duration_us'].agg(['sum', 'count', 'mean']).reset_index()
        operation_totals = operation_totals.sort_values('sum', ascending=False)
        
        total_time = operation_totals['sum'].sum()
        
        html_content = f"""
<!DOCTYPE html>
<html>
<head>
    <title>Async Events Flame Graph - Process {self.process_id[:8]}</title>
    <style>
        body {{ font-family: Arial, sans-serif; margin: 20px; }}
        .flame-bar {{ margin: 2px 0; padding: 5px; color: white; position: relative; }}
        .flame-text {{ position: absolute; left: 10px; top: 50%; transform: translateY(-50%); }}
        .stats {{ background: #f5f5f5; padding: 15px; margin: 20px 0; border-radius: 5px; }}
        h1 {{ color: #333; }}
        h2 {{ color: #666; }}
        .duration {{ font-weight: bold; }}
        .percentage {{ font-size: 0.9em; opacity: 0.8; }}
    </style>
</head>
<body>
    <h1>🔥 Async Events Flame Graph</h1>
    <div class="stats">
        <strong>Process ID:</strong> {self.process_id}<br>
        <strong>Generated:</strong> {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}<br>
        <strong>Total Spans:</strong> {len(flame_data)}<br>
        <strong>Total Duration:</strong> {total_time/1000:.2f}ms
    </div>
    
    <h2>Performance by Operation (Flame Graph View)</h2>
    <div class="flame-graph">
"""
        
        # Generate flame bars
        colors = ['#e74c3c', '#3498db', '#2ecc71', '#f39c12', '#9b59b6', '#1abc9c', '#34495e']
        
        for i, (_, row) in enumerate(operation_totals.head(15).iterrows()):
            operation = row['name'].split('::')[-1] if '::' in row['name'] else row['name']
            duration_ms = row['sum'] / 1000
            percentage = (row['sum'] / total_time) * 100
            width = max(percentage, 5)  # Minimum width for visibility
            color = colors[i % len(colors)]
            
            html_content += f"""
        <div class="flame-bar" style="background-color: {color}; width: {width}%; min-width: 200px;">
            <div class="flame-text">
                <span class="duration">{operation}</span>
                <span class="percentage">({duration_ms:.2f}ms, {percentage:.1f}%)</span>
            </div>
        </div>"""
        
        html_content += """
    </div>
    
    <h2>Detailed Statistics</h2>
    <table border="1" style="border-collapse: collapse; width: 100%;">
        <tr style="background: #f8f9fa;">
            <th style="padding: 8px;">Operation</th>
            <th style="padding: 8px;">Total Duration (ms)</th>
            <th style="padding: 8px;">Executions</th>
            <th style="padding: 8px;">Avg Duration (ms)</th>
            <th style="padding: 8px;">Percentage</th>
        </tr>
"""
        
        for _, row in operation_totals.head(10).iterrows():
            operation = row['name'].split('::')[-1] if '::' in row['name'] else row['name']
            total_ms = row['sum'] / 1000
            avg_ms = row['mean'] / 1000
            percentage = (row['sum'] / total_time) * 100
            
            html_content += f"""
        <tr>
            <td style="padding: 8px;">{operation}</td>
            <td style="padding: 8px;">{total_ms:.2f}</td>
            <td style="padding: 8px;">{row['count']}</td>
            <td style="padding: 8px;">{avg_ms:.2f}</td>
            <td style="padding: 8px;">{percentage:.1f}%</td>
        </tr>"""
        
        html_content += """
    </table>
    
    <h2>Using This Data</h2>
    <ul>
        <li><strong>Gregg Format (.txt):</strong> Use with <code>flamegraph.pl</code> to generate SVG flame graphs</li>
        <li><strong>JSON Format:</strong> Use with d3-flame-graph or other web-based visualization tools</li>
        <li><strong>HTML View:</strong> This file provides an immediate overview of performance hotspots</li>
    </ul>
    
    <p><em>Focus optimization efforts on the longest bars (highest total duration operations).</em></p>
</body>
</html>
"""
        
        with open(output_file, 'w') as f:
            f.write(html_content)
    
    def _print_overview(self):
        """Print dataset overview results."""
        df = self.results['overview']
        row = df.iloc[0]
        
        print(f"  Total Events: {row['total_events']}")
        print(f"  Unique Spans: {row['unique_spans']}")
        print(f"  Event Types: {row['event_types']}")
        print(f"  Unique Targets: {row['unique_targets']}")
        print(f"  Time Range: {row['earliest_time']} to {row['latest_time']}")
    
    def _print_span_names(self):
        """Print span names distribution."""
        df = self.results['span_names']
        
        print("  Top Span Operations by Event Count:")
        for i, row in df.head(10).iterrows():
            # Extract just the operation name (last part after ::)
            operation = row['name'].split('::')[-1]
            print(f"    {i+1}. {operation} - {row['event_count']} events ({row['percentage']}%)")
    
    def _print_performance(self):
        """Print performance analysis results."""
        df = self.results['performance']
        
        print("  Performance by Operation:")
        total_time = df['total_duration_ms'].sum()
        
        for i, row in df.iterrows():
            operation = row['name'].split('::')[-1]
            percentage = (row['total_duration_ms'] / total_time) * 100
            print(f"    {i+1}. {operation}")
            print(f"       Executions: {row['execution_count']}")
            print(f"       Avg: {row['avg_duration_ms']}ms | Max: {row['max_duration_ms']}ms")
            print(f"       Total: {row['total_duration_ms']}ms ({percentage:.1f}% of total time)")
            print(f"       Variability: {row['stddev_ms']}ms stddev")
            print()
    
    def _print_slowest_spans(self):
        """Print slowest individual spans."""
        df = self.results['slowest_spans']
        
        print("  Slowest Individual Executions:")
        for i, row in df.iterrows():
            operation = row['name'].split('::')[-1]
            timestamp = row['start_time'].strftime('%H:%M:%S.%f')[:-3]
            print(f"    {i+1}. Span {row['span_id']} - {row['duration_ms']}ms")
            print(f"       Operation: {operation}")
            print(f"       Started: {timestamp}")
    
    def _print_concurrency(self):
        """Print concurrency analysis."""
        df = self.results['concurrency']
        row = df.iloc[0]
        
        print(f"  Max Concurrent Spans: {row['max_concurrent_spans']}")
        print(f"  Avg Concurrent Spans: {row['avg_concurrent_spans']}")
        print(f"  Time Windows Analyzed: {row['time_windows_analyzed']}")
    
    def generate_summary(self) -> str:
        """Generate a text summary of key findings."""
        perf_df = self.results['performance']
        overview_df = self.results['overview']
        concurrency_df = self.results['concurrency']
        
        total_events = overview_df.iloc[0]['total_events']
        total_time = perf_df['total_duration_ms'].sum()
        top_operation = perf_df.iloc[0]['name'].split('::')[-1]
        top_time = perf_df.iloc[0]['total_duration_ms']
        top_percentage = (top_time / total_time) * 100
        max_concurrency = concurrency_df.iloc[0]['max_concurrent_spans']
        
        summary = f"""
KEY FINDINGS SUMMARY
{'='*50}

Dataset: {total_events} events across {overview_df.iloc[0]['unique_spans']} spans

Performance Bottleneck:
  🔥 {top_operation}: {top_time}ms ({top_percentage:.1f}% of total time)
  
Concurrency:
  📈 Peak: {max_concurrency} simultaneous operations
  
Top Optimization Target:
  The {top_operation} operation shows the highest total duration
  and should be the primary focus for performance improvements.
"""
        return summary
    
    def generate_flame_graphs_only(self) -> List[str]:
        """Generate only flame graphs without full analysis."""
        print(f"🔥 Generating flame graphs for process: {self.process_id}")
        print("=" * 60)
        
        print("📊 Collecting flame graph data...")
        self.results['flame_data'] = self._generate_flame_data()
        
        print("🔥 Creating flame graph files...")
        flame_files = self._create_flame_graphs()
        
        if flame_files:
            print(f"\n✅ Generated {len(flame_files)} flame graph files:")
            for file_path in flame_files:
                print(f"  📊 {file_path}")
        else:
            print("❌ No flame graph data available")
        
        return flame_files


def main():
    """Main entry point for the analyzer."""
    parser = argparse.ArgumentParser(
        description='Analyze async events performance data',
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  python async_events_analyzer.py 1333745d-77e3-4399-b937-c2562d9f526f
  python async_events_analyzer.py my-process-id --summary-only
        """
    )
    
    parser.add_argument('process_id', 
                       help='Process ID to analyze async events for')
    parser.add_argument('--summary-only', action='store_true',
                       help='Show only the summary without detailed output')
    parser.add_argument('--no-flame-graphs', action='store_true',
                       help='Skip flame graph generation for faster analysis')
    parser.add_argument('--flame-only', action='store_true',
                       help='Generate only flame graphs without console output')
    
    args = parser.parse_args()
    
    try:
        # Initialize analyzer
        analyzer = AsyncEventsAnalyzer(args.process_id)
        
        # Set options
        if args.no_flame_graphs:
            analyzer.skip_flame_graphs = True
        
        # Run analysis based on mode
        if args.flame_only:
            analyzer.generate_flame_graphs_only()
        elif args.summary_only:
            print("Running quick analysis...")
            analyzer.results['overview'] = analyzer._analyze_overview()
            analyzer.results['performance'] = analyzer._analyze_performance()
            analyzer.results['concurrency'] = analyzer._analyze_concurrency()
            if not args.no_flame_graphs:
                analyzer.results['flame_data'] = analyzer._generate_flame_data()
                analyzer._create_flame_graphs()
            print(analyzer.generate_summary())
        else:
            analyzer.run_analysis()
            print(analyzer.generate_summary())
        
        print(f"\n✅ Analysis completed successfully!")
        print(f"Generated at: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
        
    except Exception as e:
        print(f"❌ Error during analysis: {e}")
        print("\nTroubleshooting:")
        print("- Ensure Micromegas services are running")
        print("- Verify the process ID exists in the data")
        print("- Check that you're in the poetry environment")
        sys.exit(1)


if __name__ == '__main__':
    main()