#!/usr/bin/env python3
"""
Test if the callback server is working properly
"""

import http.server
import socketserver
import threading
import time
import webbrowser

PORT = 48080

class TestHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        print(f"\n✅ Received callback: {self.path}")
        self.send_response(200)
        self.send_header("Content-type", "text/html")
        self.end_headers()
        self.wfile.write(b"<html><body><h1>Callback test successful!</h1></body></html>")

    def log_message(self, format, *args):
        pass

print(f"Testing callback server on port {PORT}...")
print()

# Start server
socketserver.TCPServer.allow_reuse_address = True
server = socketserver.TCPServer(("", PORT), TestHandler)

try:
    print(f"✅ Server started on http://localhost:{PORT}")
    print()
    print("Opening browser to test callback...")
    print("The browser should show 'Callback test successful!'")
    print()

    # Start server in background
    server_thread = threading.Thread(target=server.handle_request)
    server_thread.daemon = True
    server_thread.start()

    # Open browser
    time.sleep(0.5)
    webbrowser.open(f"http://localhost:{PORT}/callback?test=success")

    # Wait for callback
    server_thread.join(timeout=10)

    print()
    print("If you saw the success page in browser, the callback server works!")
    print("If not, there might be a firewall or network issue.")

finally:
    server.server_close()
