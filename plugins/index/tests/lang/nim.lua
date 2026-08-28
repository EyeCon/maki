local th = require("maki.test_helpers")
local helpers = require("tests.helpers")
local case = th.case
local idx = helpers.idx
local idx_with_meta = helpers.idx_with_meta
local has = helpers.has
local lacks = helpers.lacks

local SRC = [==[
## Module documentation
## More module docs

import std/strutils
import std/[os, strformat]
import std/sugar except collect
from std/unicode import toUpper
export std/math
include std/threads

const MaxSize* = 1024

var counter: int = 0

type
  Point* = object
    x, y: float

  Color* = enum
    red, green

  NodeRef = ref object
    next: NodeRef

  Container[T] = object of RootObj
    items: seq[T]

  Pair = tuple[name: string, age: int]

  IntAlias = int

proc add*(a, b: int): int =
  a + b

func square(x: int): int = x * x

iterator items(s: string): char =
  for c in s: yield c

template `||`(a, b: untyped): untyped =
  a or b

{.push raises: [].}

proc strict(): int = 1

{.pop.}

test "addition":
  doAssert add(1, 2) == 3
]==]

case("nim_all_sections", function()
  local out = idx(SRC, "nim")
  has(out, {
    "module doc: [1-2]",
    "imports: [1-9]",
    "std/{os, strformat, strutils, sugar, unicode/toUpper}",
    "export: std/math",
    "include: std/threads",
    "consts:",
    "const MaxSize* [11]",
    "var counter: int [13]",
    "types:",
    "object Point* [16-17]",
    "x, y: float",
    "enum Color* [19-20]",
    "red, green",
    "ref object NodeRef [22-23]",
    "next: NodeRef",
    "object Container[T] of RootObj [25-26]",
    "items: seq[T]",
    "tuple Pair [28]",
    "name: string",
    "age: int",
    "type IntAlias = int [30]",
    "fns:",
    "proc add*(a, b: int): int [32-33]",
    "func square(x: int): int [35]",
    "iterator items(s: string): char [37-38]",
    "template `||`(a, b: untyped): untyped [40-41]",
    "tests: [49]",
  })
  lacks(out, { "collect" })
end)

case("nim_generics_and_declarations", function()
  local src = [==[
proc box[T](x: T): T =
  x

method draw*(self: RootRef) {.base.} = discard

converter toInt(f: float): int = int(f)

macro debug(n: varargs[expr]): untyped = discard

var
  a, b: int
  (c, d) = unpack()

let
  greeting = "hello"
]==]
  local out = idx(src, "nim")
  has(out, {
    "fns:",
    "proc box[T](x: T): T",
    "method draw*(self: RootRef)",
    "converter toInt(f: float): int",
    "macro debug(n: varargs[expr]): untyped",
    "consts:",
    "var a, b: int",
    "var (c, d)",
    "let greeting",
  })
  lacks(out, { "discard" })
end)

case("nim_pragma_blocks_attach_to_next_item", function()
  local src = [==[
{.push raises: [].}

proc strict(): int = 1

{.pop.}

proc relaxed(): int = 2
]==]
  local out, meta = idx_with_meta(src, "nim")
  has(out, {
    "{.push raises: [].}",
    "proc strict(): int [3]",
    "proc relaxed(): int [7]",
  })
  local lines = helpers.split_lines(out)
  for i, line in ipairs(lines) do
    if line:find("proc strict", 1, true) then
      local attr = meta[i - 1]
      assert(attr == nil or attr.range == nil, "attr line should not have ranged meta")
      break
    end
  end
end)

case("nim_doc_comments_attach_to_items", function()
  local src = [==[
## Adds two numbers
proc add*(a, b: int): int =
  a + b

## A point
type
  Point* = object
    x: float
]==]
  local out = idx(src, "nim")
  has(out, {
    "proc add*(a, b: int): int [1-3]",
    "object Point* [7-8]",
  })
end)


case("nim_unusual_constructs", function()
  local src = [==[
import std/os as o
import std/sequtils, std/strutils

##[ Block
doc comment ]##

type
  Flags* {.pure.} = enum
    On, Off

proc `[]=`(v: var Vec[int], i: int, x: int) =
  v.data[i] = x

proc noParens: int = 1

func longSignature(a: int; b: string; c: float = 1.0,
                   d: bool): string =
  discard

iterator counting(n: int): int {.closure.} =
  yield 1

const pi* = 3.14'f32

var g {.global.}: seq[string] = @[]

when isMainModule:
  echo noParens()
]==]
  local out = idx(src, "nim")
  has(out, {
    "std/{os, sequtils, strutils}",
    "types:",
    "enum Flags*",
    "On, Off",
    "proc `[]=`(v: var Vec[int], i: int, x: int)",
    "proc noParens(): int",
    "func longSignature(a: int; b: string; c: float = 1.0, d: bool): string",
    "iterator counting(n: int): int",
    "const pi*",
    "var g: seq[string]",
  })
  lacks(out, { "discard", "echo" })
end)
