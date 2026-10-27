#!/usr/bin/env python3
"""Regression checks for live-test helpers; no binary, PTY or server required."""
import unittest
from unittest.mock import Mock, patch

from scroll_pty import GraphPTY
from sidebar_layout_live import Live


class PTYWaitTests(unittest.TestCase):
    def setUp(self):
        self.graph = GraphPTY.__new__(GraphPTY)
        self.graph.process = Mock(returncode=0)
        self.graph.process.poll.return_value = 0
        self.graph.idle = Mock()
        self.graph.pump = Mock()
        self.graph.diagnostic = Mock(return_value="simulated process exit")

    def test_exit_between_success_probe_and_process_poll_is_success(self):
        self.graph.process.poll.side_effect = [None, 0, 0]
        result = self.graph.wait_for(
            "quit", lambda: self.graph.process.poll() is not None)
        self.assertIs(result, True)
        self.assertEqual(self.graph.process.poll.call_count, 3)

    def test_exit_drains_final_output_before_rechecking_success(self):
        ready = False
        value = object()

        def drained(seconds):
            nonlocal ready
            ready = True

        self.graph.idle.side_effect = drained
        result = self.graph.wait_for("final output", lambda: value if ready else None)
        self.assertIs(result, value)
        self.graph.idle.assert_called_once_with(0.05)

    def test_exit_without_the_requested_condition_still_fails(self):
        for returncode in (0, 1):
            with self.subTest(returncode=returncode):
                self.graph.process.poll.return_value = returncode
                with self.assertRaisesRegex(AssertionError, "Exited waiting for missing output"):
                    self.graph.wait_for("missing output", lambda: False)
        self.graph.pump.assert_not_called()

    def test_quit_still_rejects_nonzero_exit(self):
        self.graph.process.poll.return_value = 1
        self.graph.process.returncode = 1
        with self.assertRaisesRegex(AssertionError, "Unclean exit"):
            self.graph.quit()

    def test_ready_probe_returns_without_waiting(self):
        value = object()
        self.assertIs(self.graph.wait_for("ready", lambda: value), value)
        self.graph.process.poll.assert_not_called()
        self.graph.idle.assert_not_called()


class SidebarProcessTests(unittest.TestCase):
    def setUp(self):
        self.pid = 27422
        self.graph = {"pane_id": "test-pane"}
        self.snapshot = {
            "shell_pid": self.pid,
            "foreground_process_group_id": self.pid,
            "foreground": [(self.pid, "herdr-git-graph")],
        }
        self.identity = {
            "main_pid": self.pid,
            "shell_pid": self.pid,
            "foreground_process_group_id": self.pid,
        }
        self.expected = {"identity": self.identity, "snapshot": self.snapshot}
        self.live = Live.__new__(Live)
        self.live.process = Mock(return_value=self.snapshot)

    def test_group_leader_is_selected_among_same_named_processes(self):
        for child in (100, 27943):
            with self.subTest(child=child):
                snapshot = dict(self.snapshot, foreground=sorted(
                    self.snapshot["foreground"] + [(child, "herdr-git-graph")]))
                self.assertEqual(Live.sidebar_identity(snapshot), self.identity)
                self.live.process.return_value = snapshot
                with patch("builtins.print"):
                    self.live.assert_sidebar_process(self.graph, self.expected)

    def test_unrelated_workers_do_not_change_the_identity(self):
        snapshot = dict(self.snapshot, foreground=self.snapshot["foreground"] +
                        [(27943, "git"), (27944, "herdr")])
        self.assertEqual(Live.sidebar_identity(snapshot), self.identity)

    def test_same_named_child_cannot_replace_the_missing_main_pid(self):
        self.live.process.return_value = dict(
            self.snapshot, foreground=[(27943, "herdr-git-graph")])
        with self.assertRaisesRegex(AssertionError, "PID changed"):
            self.live.assert_sidebar_process(self.graph, self.expected)

    def test_changed_shell_or_process_group_still_fails(self):
        for field in ("shell_pid", "foreground_process_group_id"):
            with self.subTest(field=field):
                self.live.process.return_value = dict(self.snapshot, **{field: 27943})
                with self.assertRaisesRegex(AssertionError, "PID changed"):
                    self.live.assert_sidebar_process(self.graph, self.expected)

    def test_restarted_viewer_still_fails(self):
        self.live.process.return_value = {
            "shell_pid": 27943,
            "foreground_process_group_id": 27943,
            "foreground": [(27943, "herdr-git-graph")],
        }
        with self.assertRaisesRegex(AssertionError, "PID changed"):
            self.live.assert_sidebar_process(self.graph, self.expected)


if __name__ == "__main__":
    unittest.main(verbosity=2)
