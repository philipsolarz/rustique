"""Minimal perf.data reader and ELF symbolizer for benches/atlas/layers.py.

Reads PERF_RECORD_SAMPLE (ip, tid, time, period, callchain, user regs, user stack dump) and
PERF_RECORD_MMAP/MMAP2/COMM records straight from a perf.data file written by
`perf record` (file mode, uncompressed). Only what layers.py needs; no perf script.

Symbolization: (pid, ip) -> mapping (MMAP2) -> file offset -> ELF vaddr through the PT_LOAD
headers -> `nm -n -S` symbol (symtab, else dynsym). This handles the BOLT-relocated .text of
python-build-standalone binaries. Perf maps (/tmp/perf-<pid>.map, written by `-X perf`)
symbolize the Python trampolines as `py::<qualname>:<filename>`.
"""

from __future__ import annotations

import bisect
import mmap
import os
import struct
import subprocess

# perf_event_attr.sample_type bits
S_IP, S_TID, S_TIME, S_ADDR, S_READ, S_CALLCHAIN, S_ID, S_CPU, S_PERIOD, S_STREAM_ID, S_RAW, \
    S_BRANCH_STACK, S_REGS_USER, S_STACK_USER, S_WEIGHT, S_DATA_SRC, S_IDENTIFIER, \
    S_TRANSACTION, S_REGS_INTR, S_PHYS_ADDR, S_AUX, S_CGROUP, S_DATA_PAGE_SIZE, \
    S_CODE_PAGE_SIZE, S_WEIGHT_STRUCT = [1 << i for i in range(25)]

REC_MMAP, REC_COMM, REC_EXIT, REC_FORK, REC_SAMPLE, REC_MMAP2 = 1, 3, 4, 7, 9, 10
PERF_CONTEXT_MAX = 0xFFFFFFFFFFFFF000  # callchain context markers are >= this (-4095..-1)
PERF_CONTEXT_KERNEL = 0xFFFFFFFFFFFFFF80
PERF_CONTEXT_USER = 0xFFFFFFFFFFFFFE00
KERNEL_START = 0xFFFF800000000000

# x86_64 perf register numbers
REG_NAMES = ["ax", "bx", "cx", "dx", "si", "di", "bp", "sp", "ip", "flags", "cs", "ss", "ds",
             "es", "fs", "gs", "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15"]


class Attr:
    def __init__(self, raw: bytes):
        (self.type, self.size, self.config, self.sample_period, self.sample_type,
         self.read_format, self.flags) = struct.unpack_from("<IIQQQQQ", raw, 0)
        self.branch_sample_type = struct.unpack_from("<Q", raw, 72)[0] if len(raw) >= 80 else 0
        self.sample_regs_user = struct.unpack_from("<Q", raw, 80)[0] if len(raw) >= 88 else 0
        self.sample_stack_user = struct.unpack_from("<I", raw, 88)[0] if len(raw) >= 92 else 0
        self.sample_regs_intr = struct.unpack_from("<Q", raw, 96)[0] if len(raw) >= 104 else 0
        self.sample_id_all = bool(self.flags & (1 << 18))
        self.nregs_user = bin(self.sample_regs_user).count("1")
        self.nregs_intr = bin(self.sample_regs_intr).count("1")
        self.reg_index = {}
        k = 0
        for bit in range(64):
            if self.sample_regs_user & (1 << bit):
                if bit < len(REG_NAMES):
                    self.reg_index[REG_NAMES[bit]] = k
                k += 1


class Sample:
    __slots__ = ("pid", "tid", "time", "period", "ip", "callchain", "regs", "stack", "attr")

    def __init__(self):
        self.pid = self.tid = self.time = 0
        self.period = 1
        self.ip = 0
        self.callchain = None
        self.regs = None
        self.stack = None
        self.attr = None


class PerfData:
    """Iterate the records of a perf.data file."""

    def __init__(self, path):
        self.path = path
        self.f = open(path, "rb")
        self.mm = mmap.mmap(self.f.fileno(), 0, access=mmap.ACCESS_READ)
        magic = self.mm[:8]
        if magic != b"PERFILE2":
            raise ValueError(f"{path}: not a perf.data file (or pipe mode): {magic!r}")
        (_hsize, attr_size, attrs_off, attrs_size, data_off, data_size) = struct.unpack_from(
            "<QQQQQQ", self.mm, 8)
        self.data_off, self.data_size = data_off, data_size
        self.attrs = []
        self.id2attr = {}
        n = attrs_size // attr_size
        for i in range(n):
            base = attrs_off + i * attr_size
            raw = self.mm[base:base + attr_size - 16]
            a = Attr(raw)
            ids_off, ids_size = struct.unpack_from("<QQ", self.mm, base + attr_size - 16)
            for j in range(ids_size // 8):
                self.id2attr[struct.unpack_from("<Q", self.mm, ids_off + 8 * j)[0]] = a
            self.attrs.append(a)
        # with several events, samples carry PERF_SAMPLE_IDENTIFIER first
        self.multi = len(self.attrs) > 1

    def close(self):
        self.mm.close()
        self.f.close()

    def records(self):
        mm = self.mm
        pos = self.data_off
        end = self.data_off + self.data_size
        unpack = struct.unpack_from
        while pos + 8 <= end:
            rtype, _misc, size = unpack("<IHH", mm, pos)
            if size == 0:
                break
            yield rtype, pos + 8, pos + size
            pos += size

    def sample(self, start, stop):
        mm = self.mm
        unpack = struct.unpack_from
        p = start
        if self.multi:
            sid = unpack("<Q", mm, p)[0]
            attr = self.id2attr.get(sid, self.attrs[0])
        else:
            attr = self.attrs[0]
        st = attr.sample_type
        s = Sample()
        s.attr = attr
        if st & S_IDENTIFIER:
            p += 8
        if st & S_IP:
            s.ip = unpack("<Q", mm, p)[0]
            p += 8
        if st & S_TID:
            s.pid, s.tid = unpack("<II", mm, p)
            p += 8
        if st & S_TIME:
            s.time = unpack("<Q", mm, p)[0]
            p += 8
        if st & S_ADDR:
            p += 8
        if st & S_ID:
            p += 8
        if st & S_STREAM_ID:
            p += 8
        if st & S_CPU:
            p += 8
        if st & S_PERIOD:
            s.period = unpack("<Q", mm, p)[0]
            p += 8
        if st & S_READ:
            rf = attr.read_format
            if rf & 8:  # PERF_FORMAT_GROUP
                nr = unpack("<Q", mm, p)[0]
                p += 8
                if rf & 1:
                    p += 8
                if rf & 2:
                    p += 8
                per = 8 + (8 if rf & 4 else 0) + (8 if rf & 16 else 0)
                p += nr * per
            else:
                p += 8 + (8 if rf & 1 else 0) + (8 if rf & 2 else 0) + (8 if rf & 4 else 0) \
                    + (8 if rf & 16 else 0)
        if st & S_CALLCHAIN:
            nr = unpack("<Q", mm, p)[0]
            p += 8
            s.callchain = unpack(f"<{nr}Q", mm, p)
            p += 8 * nr
        if st & S_RAW:
            sz = unpack("<I", mm, p)[0]
            p += 4 + sz
        if st & S_BRANCH_STACK:
            nr = unpack("<Q", mm, p)[0]
            p += 8
            if attr.branch_sample_type & (1 << 17):  # PERF_SAMPLE_BRANCH_HW_INDEX
                p += 8
            p += 24 * nr
        if st & S_REGS_USER:
            abi = unpack("<Q", mm, p)[0]
            p += 8
            if abi:
                s.regs = unpack(f"<{attr.nregs_user}Q", mm, p)
                p += 8 * attr.nregs_user
        if st & S_STACK_USER:
            sz = unpack("<Q", mm, p)[0]
            p += 8
            if sz:
                data_pos = p
                p += sz
                dyn = unpack("<Q", mm, p)[0]
                p += 8
                s.stack = mm[data_pos:data_pos + min(dyn, sz)]
        return s

    def mmap_record(self, rtype, start):
        mm = self.mm
        if rtype == REC_MMAP2:
            pid, tid, addr, length, pgoff = struct.unpack_from("<IIQQQ", mm, start)
            fn_pos = start + 4 + 4 + 8 + 8 + 8 + 24 + 4 + 4
        else:
            pid, tid, addr, length, pgoff = struct.unpack_from("<IIQQQ", mm, start)
            fn_pos = start + 32
        nul = mm.find(b"\0", fn_pos)
        name = mm[fn_pos:nul].decode("utf-8", "replace")
        return pid, addr, length, pgoff, name


class Maps:
    """Per-pid address space from MMAP/MMAP2 records (executable or not)."""

    def __init__(self):
        self.by_pid = {}

    def add(self, pid, addr, length, pgoff, name):
        lst = self.by_pid.setdefault(pid, [])
        lst.append((addr, addr + length, pgoff, name))
        self.by_pid[pid] = lst
        self._sorted = None

    def finalize(self):
        self.index = {}
        for pid, lst in self.by_pid.items():
            lst.sort()
            self.index[pid] = ([m[0] for m in lst], lst)

    def find(self, pid, ip):
        ent = self.index.get(pid)
        if ent is None:
            # forks inherit the parent's maps; fall back to any pid that maps this address
            for ent2 in self.index.values():
                r = self._find_in(ent2, ip)
                if r:
                    return r
            return None
        return self._find_in(ent, ip)

    @staticmethod
    def _find_in(ent, ip):
        starts, lst = ent
        i = bisect.bisect_right(starts, ip) - 1
        # later mappings may overlap earlier ones; scan back a little
        for j in range(i, max(i - 8, -1), -1):
            a, b, pgoff, name = lst[j]
            if a <= ip < b:
                return a, pgoff, name
        return None


class ElfSyms:
    def __init__(self, path):
        self.path = path
        self.loads = []
        try:
            txt = subprocess.run(["readelf", "-lW", path], capture_output=True, text=True).stdout
        except OSError:
            txt = ""
        for line in txt.splitlines():
            p = line.split()
            if p and p[0] == "LOAD":
                off, vaddr, filesz = int(p[1], 16), int(p[2], 16), int(p[4], 16)
                self.loads.append((off, vaddr, filesz))
        self.addrs, self.ends, self.names = [], [], []
        for args in (["nm", "-n", "-S", "--defined-only", path],
                     ["nm", "-D", "-n", "-S", "--defined-only", path]):
            try:
                out = subprocess.run(args, capture_output=True, text=True).stdout
            except OSError:
                out = ""
            rows = []
            for line in out.splitlines():
                p = line.split()
                if len(p) == 4 and p[2] in "tTwWiI":
                    rows.append((int(p[0], 16), int(p[1], 16), p[3]))
                elif len(p) == 3 and p[1] in "tTwWiI":
                    rows.append((int(p[0], 16), 0, p[2]))
            if rows:
                rows.sort()
                # sized symbols win over unsized ones at the same address
                self.addrs = [r[0] for r in rows]
                self.ends = [r[0] + (r[1] or 1 << 16) for r in rows]
                self.names = [r[2] for r in rows]
                break

    def lookup_off(self, off):
        vaddr = None
        for o, v, sz in self.loads:
            if o <= off < o + sz:
                vaddr = off - o + v
                break
        if vaddr is None:
            vaddr = off
        i = bisect.bisect_right(self.addrs, vaddr) - 1
        # an unsized symbol may follow a sized one at the same address; prefer one containing
        while i >= 0 and vaddr >= self.ends[i]:
            if i > 0 and self.addrs[i - 1] == self.addrs[i]:
                i -= 1
                continue
            return None
        if i < 0:
            return None
        return self.names[i]


class PerfMap:
    """/tmp/perf-<pid>.map: '<start> <size> <name>' lines (hex)."""

    def __init__(self, path):
        rows = []
        with open(path, errors="replace") as f:
            for line in f:
                parts = line.rstrip("\n").split(" ", 2)
                if len(parts) == 3:
                    try:
                        rows.append((int(parts[0], 16), int(parts[1], 16), parts[2]))
                    except ValueError:
                        pass
        rows.sort()
        self.starts = [r[0] for r in rows]
        self.rows = rows

    def lookup(self, ip):
        i = bisect.bisect_right(self.starts, ip) - 1
        if i >= 0:
            a, n, name = self.rows[i]
            if a <= ip < a + n:
                return name
        return None


class Symbolizer:
    def __init__(self, maps: Maps, perf_maps: dict | None = None):
        self.maps = maps
        self.elfs = {}
        self.cache = {}
        self.perf_maps = perf_maps or {}

    def sym(self, pid, ip, is_return_addr):
        """-> (symbol or None, dso path or tag)."""
        key = (pid, ip, is_return_addr)
        r = self.cache.get(key)
        if r is not None:
            return r
        if ip >= KERNEL_START:
            r = ("[kernel]", "[kernel]")
        else:
            m = self.maps.find(pid, ip)
            if m is None:
                pm = self.perf_maps.get(pid)
                name = pm.lookup(ip) if pm else None
                r = (name, "[perf-map]" if name else "[unknown]")
            else:
                start, pgoff, path = m
                if path.startswith("/tmp/perf-") or path.startswith("//anon") or path.startswith("["):
                    pm = self.perf_maps.get(pid)
                    name = pm.lookup(ip) if pm else None
                    r = (name, "[perf-map]" if name else path)
                else:
                    off = ip - start + pgoff
                    if is_return_addr and off > 0:
                        off -= 1
                    e = self.elfs.get(path)
                    if e is None:
                        e = self.elfs[path] = ElfSyms(path) if os.path.isfile(path) else False
                    r = (e.lookup_off(off) if e else None, path)
        self.cache[key] = r
        return r


def fp_unwind(sample, max_frames=256):
    """Frame-pointer unwind of a user stack dump: -> [(ip, frame_record_addr or None)].

    The first entry is the sampled user IP. Each further entry is a return address read from
    [rbp+8] while rbp lies inside the dumped stack."""
    regs = sample.regs
    stack = sample.stack
    ri = sample.attr.reg_index
    if regs is None:
        return []
    ip = regs[ri["ip"]]
    sp = regs[ri["sp"]]
    bp = regs[ri["bp"]]
    out = [(ip, None)]
    if not stack:
        return out
    n = len(stack)
    unpack = struct.unpack_from
    seen = 0
    while seen < max_frames:
        off = bp - sp
        if off < 0 or off + 16 > n:
            break
        nbp, ret = unpack("<QQ", stack, off)
        if ret == 0:
            break
        out.append((ret, bp))
        if nbp <= bp:
            break
        bp = nbp
        seen += 1
    return out
