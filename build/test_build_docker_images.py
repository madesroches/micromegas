#!/usr/bin/env python3
"""Unit tests for build_docker_images.py's tag/command/revision logic.

These run with no Docker and no real git history: `get_revision` is exercised against a
throwaway `git init` repo in `tmp_path`, and everything else is pure functions or a
monkeypatched `run_command`.
"""

import argparse
import subprocess
import sys

import pytest

from build_docker_images import (
    build_command,
    build_image,
    failed_results,
    get_revision,
    image_tags,
    main,
    rerun_command,
)

FULL_SHA = "6a6822cb3f1ecba1234567890abcdef123456789"


def init_git_repo(path):
    def run(*args):
        subprocess.run(["git", *args], cwd=path, check=True, capture_output=True)

    run("init")
    run("config", "user.name", "Test")
    run("config", "user.email", "test@example.com")
    (path / "tracked.txt").write_text("hello\n")
    run("add", "tracked.txt")
    run("commit", "-m", "initial commit")


class TestImageTags:
    def test_clean_amd64(self):
        assert image_tags("1.2.3", FULL_SHA, arm64=False) == [
            "1.2.3",
            "latest",
            FULL_SHA[:12],
        ]

    def test_clean_arm64(self):
        assert image_tags("1.2.3", FULL_SHA, arm64=True) == [
            "1.2.3-arm64",
            "latest-arm64",
            f"{FULL_SHA[:12]}-arm64",
        ]

    def test_dirty_amd64(self):
        assert image_tags("1.2.3", f"{FULL_SHA}-dirty", arm64=False) == [
            "1.2.3",
            "latest",
            f"{FULL_SHA[:12]}-dirty",
        ]

    def test_dirty_arm64(self):
        assert image_tags("1.2.3", f"{FULL_SHA}-dirty", arm64=True) == [
            "1.2.3-arm64",
            "latest-arm64",
            f"{FULL_SHA[:12]}-dirty-arm64",
        ]


class TestBuildCommand:
    def test_amd64(self):
        cmd = build_command(
            "ingestion.Dockerfile",
            "user/repo-ingestion",
            ["1.2.3", "latest", "6a6822cb3f1e"],
            FULL_SHA,
            arm64=False,
            push=False,
        )
        assert cmd[:2] == ["docker", "build"]
        assert "buildx" not in cmd
        assert cmd.count("-t") == 3
        assert "user/repo-ingestion:1.2.3" in cmd
        assert "user/repo-ingestion:latest" in cmd
        assert "user/repo-ingestion:6a6822cb3f1e" in cmd
        assert "--label" in cmd
        label_index = cmd.index("--label")
        assert cmd[label_index + 1] == f"org.opencontainers.image.revision={FULL_SHA}"
        assert cmd[-1] == "."

    def test_arm64_load(self):
        cmd = build_command(
            "ingestion.Dockerfile",
            "user/repo-ingestion",
            ["1.2.3-arm64", "latest-arm64", "6a6822cb3f1e-arm64"],
            FULL_SHA,
            arm64=True,
            push=False,
        )
        assert cmd[:6] == [
            "docker",
            "buildx",
            "build",
            "--platform",
            "linux/arm64",
            "--load",
        ]
        assert "--push" not in cmd
        assert cmd.count("-t") == 3
        assert cmd[-1] == "."
        label_index = cmd.index("--label")
        assert cmd[label_index + 1] == f"org.opencontainers.image.revision={FULL_SHA}"

    def test_arm64_push(self):
        cmd = build_command(
            "ingestion.Dockerfile",
            "user/repo-ingestion",
            ["1.2.3-arm64", "latest-arm64", "6a6822cb3f1e-arm64"],
            FULL_SHA,
            arm64=True,
            push=True,
        )
        assert cmd[:6] == [
            "docker",
            "buildx",
            "build",
            "--platform",
            "linux/arm64",
            "--push",
        ]
        assert "--load" not in cmd
        assert cmd.count("-t") == 3
        assert cmd[-1] == "."
        label_index = cmd.index("--label")
        assert cmd[label_index + 1] == f"org.opencontainers.image.revision={FULL_SHA}"


class TestGetRevision:
    def test_clean_repo(self, tmp_path):
        init_git_repo(tmp_path)
        head = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=tmp_path,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        assert get_revision(tmp_path) == head

    def test_dirty_from_modified_tracked_file(self, tmp_path):
        init_git_repo(tmp_path)
        (tmp_path / "tracked.txt").write_text("modified\n")
        assert get_revision(tmp_path).endswith("-dirty")

    def test_dirty_from_untracked_file(self, tmp_path):
        init_git_repo(tmp_path)
        (tmp_path / "untracked.txt").write_text("new\n")
        assert get_revision(tmp_path).endswith("-dirty")

    def test_non_repo_raises(self, tmp_path):
        with pytest.raises(subprocess.CalledProcessError):
            get_revision(tmp_path)


class TestBuildImagePushLoop:
    def test_amd64_push_pushes_every_tag(self, monkeypatch):
        calls = []

        def fake_run_command(cmd, cwd=None):
            calls.append(cmd)
            return True

        monkeypatch.setattr("build_docker_images.run_command", fake_run_command)

        result = build_image(
            "redis-exporter", "1.2.3", FULL_SHA, push=True, arm64=False
        )

        assert result["built"] is True
        assert result["pushed"] is True
        assert result["arm64"] is False

        push_calls = [c for c in calls if c[:2] == ["docker", "push"]]
        assert len(push_calls) == 3
        pushed_images = {c[2] for c in push_calls}
        assert pushed_images == {
            f"marcantoinedesroches/micromegas-redis-exporter:{tag}"
            for tag in result["tags"]
        }


def result(service, arm64=False, built=True, pushed=True):
    return {
        "service": service,
        "image": f"u/r-{service}",
        "tags": ["1.2.3"],
        "arm64": arm64,
        "built": built,
        "pushed": pushed,
    }


def ns(push=False, version=None):
    return argparse.Namespace(push=push, version=version)


class TestFailedResults:
    def test_all_ok(self):
        assert failed_results([result("a"), result("b", arm64=True)], push=True) == []

    def test_build_failure(self):
        bad = result("a", built=False, pushed=False)
        assert failed_results([result("b"), bad], push=False) == [bad]

    def test_push_failure_only_with_push(self):
        bad = result("a", built=True, pushed=False)
        assert failed_results([bad], push=True) == [bad]
        assert failed_results([bad], push=False) == []


class TestRerunCommand:
    def test_amd64_only(self):
        cmd = rerun_command([result("a"), result("b")], ns())
        assert cmd == "python3 build/build_docker_images.py a b"

    def test_arm64_only(self):
        cmd = rerun_command([result("a", arm64=True)], ns())
        assert cmd == "python3 build/build_docker_images.py --arm64 a"

    def test_mixed_arches_two_lines(self):
        cmd = rerun_command([result("a"), result("b", arm64=True)], ns())
        assert cmd.splitlines() == [
            "python3 build/build_docker_images.py a",
            "python3 build/build_docker_images.py --arm64 b",
        ]

    def test_push_and_version_carried_over(self):
        cmd = rerun_command([result("a")], ns(push=True, version="9.9.9"))
        assert cmd == "python3 build/build_docker_images.py --push --version 9.9.9 a"

    def test_no_version_when_not_given(self):
        assert "--version" not in rerun_command([result("a")], ns(push=True))


class TestBuildImageArch:
    def test_arm64_key(self, monkeypatch):
        monkeypatch.setattr(
            "build_docker_images.run_command", lambda cmd, cwd=None: True
        )
        r = build_image("redis-exporter", "1.2.3", FULL_SHA, push=True, arm64=True)
        assert r["arm64"] is True
        assert r["pushed"] is True


class TestMainExitCode:
    def run_main(self, monkeypatch, argv, pushed):
        monkeypatch.setattr(sys, "argv", ["build_docker_images.py", *argv])
        monkeypatch.setattr("build_docker_images.get_revision", lambda: FULL_SHA)
        monkeypatch.setattr(
            "build_docker_images.build_image",
            lambda service, version, revision, push, arm64: result(
                service, arm64=arm64, pushed=pushed
            ),
        )
        return main()

    def test_push_failure_exits_nonzero(self, monkeypatch, capsys):
        code = self.run_main(
            monkeypatch, ["--push", "--version", "1.2.3", "ingestion"], pushed=False
        )
        out = capsys.readouterr().out
        assert code == 1
        assert "ingestion (amd64): not pushed" in out
        assert (
            "python3 build/build_docker_images.py --push --version 1.2.3 ingestion"
            in out
        )
        assert out.rstrip().endswith("FAILED: 1 image(s)")

    def test_arm64_push_failure_says_build_push_failed(self, monkeypatch, capsys):
        monkeypatch.setattr(
            sys, "argv", ["build_docker_images.py", "--arm64", "--push", "ingestion"]
        )
        monkeypatch.setattr("build_docker_images.get_revision", lambda: FULL_SHA)
        monkeypatch.setattr(
            "build_docker_images.build_image",
            lambda service, version, revision, push, arm64: result(
                service, arm64=arm64, built=False
            ),
        )
        assert main() == 1
        assert "ingestion (arm64): build/push failed" in capsys.readouterr().out

    def test_success_exits_zero(self, monkeypatch):
        code = self.run_main(
            monkeypatch, ["--push", "--version", "1.2.3", "ingestion"], pushed=True
        )
        assert code == 0


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-v"]))
