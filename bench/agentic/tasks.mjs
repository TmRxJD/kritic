// Tasks for the agentic token benchmark. Each is a small Python project, a request an agent gets,
// and hidden tests the agent never sees. The visible test file covers the happy path only, as a
// real repo's often does; the hidden tests check the edge cases a careful review should catch.
// Ground truth is the hidden tests alone: no arm's own opinion counts.

const t = (s) => s.replace(/^\n/, "");

export const TASKS = [
  {
    id: "slugify",
    prompt: "Implement `slugify(title)` in text.py: lowercase, ASCII only (strip accents, e.g. é -> e), words joined by single hyphens, no leading or trailing hyphens, and at most 60 characters without cutting a word in half. Keep the existing test passing.",
    files: {
      "text.py": "def slugify(title):\n    raise NotImplementedError\n",
      "test_text.py": t(`
import unittest
from text import slugify

class T(unittest.TestCase):
    def test_basic(self):
        self.assertEqual(slugify("Hello World"), "hello-world")

if __name__ == "__main__":
    unittest.main()
`),
    },
    hidden: t(`
import unittest
from text import slugify

class Hidden(unittest.TestCase):
    def test_accents(self):
        self.assertEqual(slugify("Crème Brûlée à la carte"), "creme-brulee-a-la-carte")
    def test_punctuation_runs(self):
        self.assertEqual(slugify("  --Hello,,   World!!--  "), "hello-world")
    def test_length_word_boundary(self):
        s = slugify("word " * 30)
        self.assertLessEqual(len(s), 60)
        self.assertFalse(s.endswith("-"))
        self.assertTrue(all(p == "word" for p in s.split("-")))
    def test_empty(self):
        self.assertEqual(slugify("!!!"), "")
    def test_non_latin_dropped(self):
        self.assertEqual(slugify("日本 tokyo"), "tokyo")
`),
  },
  {
    id: "duration",
    prompt: "Implement `parse_duration(text)` in duration.py returning total seconds as an int for strings like '1h30m', '45s', '2h', '1h5m10s'. Units h, m, s, each at most once, in that order. Raise ValueError for anything else (empty, unknown unit, repeated or out-of-order units, missing numbers). Keep the existing test passing.",
    files: {
      "duration.py": "def parse_duration(text):\n    raise NotImplementedError\n",
      "test_duration.py": t(`
import unittest
from duration import parse_duration

class T(unittest.TestCase):
    def test_basic(self):
        self.assertEqual(parse_duration("1h30m"), 5400)

if __name__ == "__main__":
    unittest.main()
`),
    },
    hidden: t(`
import unittest
from duration import parse_duration

class Hidden(unittest.TestCase):
    def test_all_units(self):
        self.assertEqual(parse_duration("1h5m10s"), 3910)
        self.assertEqual(parse_duration("45s"), 45)
        self.assertEqual(parse_duration("2h"), 7200)
    def test_rejects(self):
        for bad in ["", "h", "10", "1x", "1m1h", "1h1h", "1.5h", "-1h", "1h 30m", "m30"]:
            with self.assertRaises(ValueError, msg=bad):
                parse_duration(bad)
`),
  },
  {
    id: "user-search",
    prompt: "Implement `find_users(conn, name_prefix, limit=20)` in users.py: return a list of (id, name) tuples from the sqlite3 `users` table whose name starts with name_prefix (case-insensitive), ordered by name, at most `limit` rows (limit must be 1-100, else ValueError). Keep the existing test passing.",
    files: {
      "users.py": "def find_users(conn, name_prefix, limit=20):\n    raise NotImplementedError\n",
      "test_users.py": t(`
import sqlite3, unittest
from users import find_users

def db():
    c = sqlite3.connect(":memory:")
    c.execute("create table users (id integer primary key, name text)")
    c.executemany("insert into users (name) values (?)", [("alice",), ("Albert",), ("bob",)])
    return c

class T(unittest.TestCase):
    def test_prefix(self):
        self.assertEqual([n for _, n in find_users(db(), "al")], ["Albert", "alice"])

if __name__ == "__main__":
    unittest.main()
`),
    },
    hidden: t(`
import sqlite3, unittest
from users import find_users

def db():
    c = sqlite3.connect(":memory:")
    c.execute("create table users (id integer primary key, name text)")
    c.executemany("insert into users (name) values (?)", [("alice",), ("Albert",), ("bob",), ("al%x",), ("al_y",), ("o'neil",)])
    return c

class Hidden(unittest.TestCase):
    def test_injection_is_data(self):
        self.assertEqual(find_users(db(), "' or 1=1 --"), [])
        self.assertEqual([n for _, n in find_users(db(), "o'")], ["o'neil"])
    def test_wildcards_literal(self):
        self.assertEqual([n for _, n in find_users(db(), "al%")], ["al%x"])
        self.assertEqual([n for _, n in find_users(db(), "al_")], ["al_y"])
    def test_limit(self):
        self.assertEqual(len(find_users(db(), "", 2)), 2)
        for bad in [0, 101, -1]:
            with self.assertRaises(ValueError):
                find_users(db(), "a", bad)
`),
  },
  {
    id: "lru",
    prompt: "Implement class `LRUCache(capacity)` in cache.py with get(key, default=None) and put(key, value). Evict the least recently used entry when full; get and put both count as use. capacity must be a positive int (ValueError otherwise). Keep the existing test passing.",
    files: {
      "cache.py": "class LRUCache:\n    def __init__(self, capacity):\n        raise NotImplementedError\n",
      "test_cache.py": t(`
import unittest
from cache import LRUCache

class T(unittest.TestCase):
    def test_basic(self):
        c = LRUCache(2)
        c.put("a", 1)
        self.assertEqual(c.get("a"), 1)

if __name__ == "__main__":
    unittest.main()
`),
    },
    hidden: t(`
import unittest
from cache import LRUCache

class Hidden(unittest.TestCase):
    def test_eviction_order(self):
        c = LRUCache(2)
        c.put("a", 1); c.put("b", 2); c.get("a"); c.put("c", 3)
        self.assertIsNone(c.get("b"))
        self.assertEqual((c.get("a"), c.get("c")), (1, 3))
    def test_put_refreshes(self):
        c = LRUCache(2)
        c.put("a", 1); c.put("b", 2); c.put("a", 10); c.put("c", 3)
        self.assertEqual(c.get("a"), 10)
        self.assertIsNone(c.get("b"))
    def test_default_and_none_values(self):
        c = LRUCache(1)
        c.put("k", None)
        self.assertIsNone(c.get("k", "d"))
        self.assertEqual(c.get("x", "d"), "d")
    def test_bounded(self):
        c = LRUCache(3)
        for i in range(1000):
            c.put(i, i)
        self.assertEqual(sum(1 for i in range(1000) if c.get(i) is not None), 3)
    def test_capacity(self):
        for bad in [0, -1, 1.5, "2"]:
            with self.assertRaises(ValueError):
                LRUCache(bad)
`),
  },
  {
    id: "csv-row",
    prompt: "Implement `parse_row(line)` in csvrow.py without the csv module: split one CSV line into fields. Fields may be quoted with double quotes; inside quotes commas are literal and a doubled quote \"\" is one quote. Unquoted fields are kept as is (no trimming). Raise ValueError on an unterminated quote. Keep the existing test passing.",
    files: {
      "csvrow.py": "def parse_row(line):\n    raise NotImplementedError\n",
      "test_csvrow.py": t(`
import unittest
from csvrow import parse_row

class T(unittest.TestCase):
    def test_basic(self):
        self.assertEqual(parse_row("a,b,c"), ["a", "b", "c"])

if __name__ == "__main__":
    unittest.main()
`),
    },
    hidden: t(`
import unittest
from csvrow import parse_row

class Hidden(unittest.TestCase):
    def test_quoted(self):
        self.assertEqual(parse_row('"a,b",c'), ["a,b", "c"])
        self.assertEqual(parse_row('"say ""hi""",x'), ['say "hi"', "x"])
    def test_empty_fields(self):
        self.assertEqual(parse_row(",,"), ["", "", ""])
        self.assertEqual(parse_row(""), [""])
        self.assertEqual(parse_row('""'), [""])
    def test_spaces_kept(self):
        self.assertEqual(parse_row(" a , b "), [" a ", " b "])
    def test_unterminated(self):
        with self.assertRaises(ValueError):
            parse_row('"abc,d')
`),
  },
  {
    id: "overlap",
    prompt: "Implement `merge_ranges(ranges)` in ranges.py: given a list of (start, end) integer pairs (half-open, start < end required, else ValueError), return the sorted list of merged ranges, merging ranges that overlap or touch. Do not mutate the input. Keep the existing test passing.",
    files: {
      "ranges.py": "def merge_ranges(ranges):\n    raise NotImplementedError\n",
      "test_ranges.py": t(`
import unittest
from ranges import merge_ranges

class T(unittest.TestCase):
    def test_basic(self):
        self.assertEqual(merge_ranges([(1, 3), (2, 5)]), [(1, 5)])

if __name__ == "__main__":
    unittest.main()
`),
    },
    hidden: t(`
import unittest
from ranges import merge_ranges

class Hidden(unittest.TestCase):
    def test_touching_and_unsorted(self):
        self.assertEqual(merge_ranges([(5, 7), (1, 3), (3, 4)]), [(1, 4), (5, 7)])
    def test_contained(self):
        self.assertEqual(merge_ranges([(1, 10), (2, 3), (4, 5)]), [(1, 10)])
    def test_empty(self):
        self.assertEqual(merge_ranges([]), [])
    def test_no_mutation(self):
        r = [(3, 4), (1, 2)]
        merge_ranges(r)
        self.assertEqual(r, [(3, 4), (1, 2)])
    def test_invalid(self):
        for bad in [[(2, 2)], [(3, 1)]]:
            with self.assertRaises(ValueError):
                merge_ranges(bad)
`),
  },
];
