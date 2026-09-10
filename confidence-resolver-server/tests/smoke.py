#!/usr/bin/env python3
"""Exercise the packaged image using the synthetic account fixture (no credentials)."""
import argparse
import json
import shutil
import subprocess
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("state", type=Path, help="protobuf encoded tests/fixtures/account.json")
parser.add_argument("--image", default="confidence-resolver-server:local")
args = parser.parse_args()
payload = args.state.read_bytes()
reports = []


class Backend(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        self.send_response(200 if self.path == "/state" else 404)
        self.send_header("Content-Type", "application/x-protobuf")
        self.send_header("ETag", "fixture-v1")
        self.end_headers()
        self.wfile.write(payload)

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        reports.append((self.path, self.headers.get("Authorization"), len(body)))
        self.send_response(200)
        self.end_headers()


def docker(*args):
    return subprocess.check_output(["docker", *args], text=True).strip()


backend = ThreadingHTTPServer(("0.0.0.0", 0), Backend)
threading.Thread(target=backend.serve_forever, daemon=True).start()
container = None
try:
    remote = f"http://host.docker.internal:{backend.server_port}"
    container = docker(
        "run", "-d", "--read-only", "--cap-drop", "ALL",
        "--security-opt", "no-new-privileges", "--add-host", "host.docker.internal:host-gateway",
        "-p", "127.0.0.1::8090", "-p", "127.0.0.1::5990",
        "-e", f"CONFIDENCE_RESOLVER_STATE_URL={remote}/state",
        "-e", f"CONFIDENCE_RESOLVER_API_URL={remote}",
        "-e", "CONFIDENCE_ACCOUNT=accounts/test",
        "-e", "CONFIDENCE_RESOLVER_POLL_INTERVAL_SECONDS=1",
        "-e", "CONFIDENCE_ASSIGN_LOG_INTERVAL_SECONDS=1", args.image,
    )
    address = docker("port", container, "8090/tcp")

    def http(path, data=None):
        data = None if data is None else json.dumps(data).encode()
        request = urllib.request.Request(f"http://{address}{path}", data=data,
                                         headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(request, timeout=5) as response:
            return response.read()

    for attempt in range(100):
        try:
            http("/v1/health")
            break
        except (urllib.error.URLError, ConnectionError):
            time.sleep(0.1)
    else:
        raise AssertionError("Container never became ready")
    docker("exec", container, "confidence-resolver-server", "--health-check")
    for secret, flag in [("test-a", "flags/a"), ("test-ab", "flags/ab"), ("test-a", "flags/a")]:
        response = json.loads(http("/v1/flags:resolve", {
            "clientSecret": secret, "evaluationContext": {"targeting_key": "smoke-user"}, "apply": False,
        }))
        assert {f["flag"] for f in response["resolvedFlags"]} == {flag, "flags/shared"}
        http("/v1/flags:apply", {
            "clientSecret": secret, "resolveToken": response["resolveToken"],
            "sendTime": "2026-09-09T00:00:00Z",
            "flags": [{"flag": flag, "applyTime": "2026-09-09T00:00:00Z"}],
        })
    assert b"confidence_server_resolve_requests_total 3" in http("/v1/metrics")
    assert b"confidence_server_ready 1" in http("/v1/telemetry")
    if shutil.which("grpcurl"):
        grpc = docker("port", container, "5990/tcp")
        services = subprocess.check_output(["grpcurl", "-plaintext", grpc, "list"], text=True)
        assert "confidence.flags.resolver.v1.FlagResolverService" in services
        health = subprocess.check_output(["grpcurl", "-plaintext", "-d", "{}", grpc,
                                          "grpc.health.v1.Health/Check"], text=True)
        assert json.loads(health)["status"] == "SERVING"
        result = subprocess.check_output([
            "grpcurl", "-plaintext", "-d", '{"clientSecret":"test-ab","evaluationContext":{"targeting_key":"grpc-user"},"apply":true}',
            grpc, "confidence.flags.resolver.v1.FlagResolverService/ResolveFlags",
        ], text=True)
        assert {f["flag"] for f in json.loads(result)["resolvedFlags"]} == {"flags/ab", "flags/shared"}
        print("gRPC resolve, health and reflection passed")
    else:
        print("grpcurl unavailable; packaged gRPC check skipped")
    docker("stop", "--time", "35", container)
    assert docker("inspect", "--format", "{{.State.ExitCode}}", container) == "0"
    assert {auth for path, auth, size in reports if path == "/v1/clientFlagLogs:write" and size} == {
        "ClientSecret test-a", "ClientSecret test-ab",
    }
    print("Container HTTP, multi-client isolation, deferred apply, reporting, health and SIGTERM passed")
finally:
    if container:
        docker("rm", "-f", container)
    backend.shutdown()
