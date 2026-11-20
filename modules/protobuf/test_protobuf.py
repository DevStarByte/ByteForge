"""Tests for the Protocol Buffers module: python3 -m unittest discover -s modules/protobuf -p 'test_*.py' -v"""

import json
import os
import re
import shutil
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import protobuf as pb  # noqa: E402

PROTO = 'syntax = "proto3";\noption csharp_namespace="Shop.Grpc";\npackage shop;\nservice Orders{rpc Get(GetRequest)returns(Order);}\nmessage GetRequest{int32 id=1;}\nmessage Order{int32 id=1; repeated string items=2;}\n'


def manifest():
    with open(os.path.join(HERE, "module.json"), encoding="utf-8") as f:
        return json.loads(re.sub(r"(?m)^\s*//.*$", "", f.read()))


def first_token(rules, line, text):
    """The token the earliest rule gives `text` on `line` (what the IDE does: the earlier rule wins)."""
    start = line.index(text)
    for rule in rules:
        for m in re.finditer(rule["match"], line):
            group = rule.get("group", 0)
            if m.start(group) <= start < m.end(group):
                return rule["token"]
    return None


class ManifestTests(unittest.TestCase):
    def test_rules_compile_without_lookaround(self):
        language = manifest()["languages"][0]
        for pattern in [r["match"] for r in language["rules"]] + language["detect"]:
            re.compile(pattern)
            self.assertNotRegex(pattern, r"\(\?<?[=!]", "Rust's regex crate has no lookaround")

    def test_highlighting(self):
        rules = manifest()["languages"][0]["rules"]
        cases = [
            ("service Orders {", "service", "keyword"),
            ("service Orders {", "Orders", "class name"),
            ("  rpc Get (GetRequest) returns (stream Order);", "Get", "method name"),
            ("  rpc Get (GetRequest) returns (stream Order);", "GetRequest", "class name"),
            ("  rpc Get (GetRequest) returns (stream Order);", "Order", "class name"),
            ("  repeated string items = 2; // the items", "repeated", "keyword"),
            ("  repeated string items = 2; // the items", "items", "field name"),
            ("  repeated string items = 2; // the items", "2", "number"),
            ("  repeated string items = 2; // the items", "the", "comment"),
            ("package shop.orders;", "shop.orders", "namespace name"),
            ('option csharp_namespace = "Shop.Grpc";', "csharp_namespace", "property name"),
            ('option csharp_namespace = "Shop.Grpc";', "Shop.Grpc", "string"),
            ("  google.protobuf.Timestamp created = 3;", "google.protobuf.Timestamp", "class name"),
        ]
        for line, text, token in cases:
            self.assertEqual(first_token(rules, line, text), token, f"{text!r} in {line!r}")


class FormatTests(unittest.TestCase):
    def test_missing_formatter_says_what_to_install(self):
        pb.settings = {"protobuf.formatter": "no-such-formatter"}
        text, error = pb.format_proto("/tmp/a.proto", PROTO)
        self.assertIsNone(text)
        self.assertIn("install", error)

    @unittest.skipUnless(shutil.which("clang-format"), "clang-format isn't installed")
    def test_clang_format(self):
        pb.settings = {"protobuf.formatter": "clang-format"}
        text, error = pb.format_proto(None, PROTO)
        self.assertIsNone(error)
        self.assertIn("rpc Get(GetRequest) returns (Order);", text)
        self.assertIn("int32 id = 1;", text)

    @unittest.skipUnless(shutil.which("buf"), "buf isn't installed")
    def test_buf(self):
        pb.settings = {"protobuf.formatter": "buf"}
        text, error = pb.format_proto(None, PROTO)
        self.assertIsNone(error)
        self.assertIn("int32 id = 1;", text)


if __name__ == "__main__":
    unittest.main()
