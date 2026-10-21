import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))

import start_monolith_docker as m  # noqa: E402


def test_defaults():
    args = m.parse_args([])
    assert not (args.pull or args.follow or args.down or args.volumes)
    assert args.timeout == m.DEFAULT_TIMEOUT


def test_volumes_requires_down():
    with pytest.raises(SystemExit):
        m.parse_args(["--volumes"])
    assert m.parse_args(["--down", "--volumes"]).volumes


def test_compose_cmd_carries_project_and_absolute_file():
    cmd = m.build_compose_cmd("logs", "-f")
    assert cmd[:4] == ["docker", "compose", "-p", "micromegas-monolith"]
    compose_file = Path(cmd[cmd.index("-f") + 1])
    assert compose_file.is_absolute()
    assert compose_file.name == "docker-compose.monolith.yaml"
    assert compose_file.exists()
    assert cmd[-2:] == ["logs", "-f"]


def test_up_cmd_pull():
    assert "--pull" not in m.build_up_cmd(False)
    cmd = m.build_up_cmd(True)
    assert cmd[-4:] == ["up", "-d", "--pull", "always"]


def test_down_cmd_volumes():
    assert m.build_down_cmd(False)[-1] == "down"
    assert m.build_down_cmd(True)[-2:] == ["down", "-v"]


def test_summary_urls():
    text = "\n".join(msg for msg, _ in m.format_summary())
    assert "http://localhost:3000/" in text
    assert "http://localhost:9000" in text
    assert "grpc://localhost:50051" in text
    assert "micromegas-query" in text


@pytest.mark.parametrize(
    "text,ok",
    [
        ("2.22.0", False),
        ("2.23.0", False),
        ("2.23.1", True),
        ("2.24.5", True),
        ("v2.23.1", True),
        ("v2.40.0-desktop.1", True),
        ("3.0.0", True),
        ("garbage", False),
        ("", False),
    ],
)
def test_compose_version(text, ok):
    assert m.compose_version_ok(text) is ok


def test_already_running():
    assert not m.is_already_running("")
    assert not m.is_already_running("\n")
    assert m.is_already_running("abc123\n")


class FakeClock:
    def __init__(self):
        self.now = 0.0

    def sleep(self, seconds):
        self.now += seconds

    def clock(self):
        return self.now


def test_wait_for_ready_immediate():
    c = FakeClock()
    assert m.wait_for_ready(lambda: True, 10, sleep=c.sleep, clock=c.clock)
    assert c.now == 0


def test_wait_for_ready_eventual():
    c = FakeClock()
    calls = iter([False, False, True])
    assert m.wait_for_ready(lambda: next(calls), 10, sleep=c.sleep, clock=c.clock)


def test_wait_for_ready_timeout():
    c = FakeClock()
    assert not m.wait_for_ready(lambda: False, 5, sleep=c.sleep, clock=c.clock)
    assert c.now >= 5
