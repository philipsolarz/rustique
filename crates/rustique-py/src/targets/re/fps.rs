//! Fingerprints the `re` group requires (`benches/targets/re_record.py` prints them; record
//! per minor version, the free-threaded builds give the same values): every Python function
//! on the success path of `re._compiler.compile`, the module functions and `_compile`'s hit
//! path, and `parse_template` (template `sub`).

use crate::accel::Fingerprints;

pub(crate) const FPS: &[(&str, Fingerprints)] = &[
    ("re._compiler.compile", &[(3, 14, "ba62832b78132e89"), (3, 15, "7414326bcf6e289d")]),
    ("re._compiler.isstring", &[(3, 14, "db8e9d00f161e807"), (3, 15, "1f45f527bff225fe")]),
    ("re._compiler._code", &[(3, 14, "8b8726fadb67f74a"), (3, 15, "c865b41d55ed81e7")]),
    ("re._compiler._compile_info", &[(3, 14, "db9492f3e0ffcc4a"), (3, 15, "12549ed9fda3acde")]),
    ("re._compiler._compile", &[(3, 14, "7c032d7ac54e9794"), (3, 15, "5013c2f6c4d191d2")]),
    ("re._compiler._compile_charset", &[(3, 14, "6562cd4d0d909cc2"), (3, 15, "5c2f97572b2efa4a")]),
    ("re._compiler._optimize_charset", &[(3, 14, "5905620b5ed927e6"), (3, 15, "ac02e59ca4ba42a2")]),
    ("re._compiler._mk_bitmap", &[(3, 14, "e4e293c1e44c2c14"), (3, 15, "96b47e9416dff973")]),
    ("re._compiler._bytes_to_codes", &[(3, 14, "eed3eb66c97416fd"), (3, 15, "a8e6d6cb15f7e147")]),
    ("re._compiler._simple", &[(3, 14, "e8f6012fce5be0bf"), (3, 15, "374bec33360e9e30")]),
    ("re._compiler._generate_overlap_table", &[(3, 14, "b1874af5d451ff9e"), (3, 15, "441b37abd220dac6")]),
    ("re._compiler._get_iscased", &[(3, 14, "ef894e291ee763b7"), (3, 15, "ac036f48af5bbac6")]),
    ("re._compiler._get_literal_prefix", &[(3, 14, "6dde4daf4b5f82b8"), (3, 15, "89ced94d2f965dec")]),
    ("re._compiler._get_charset_prefix", &[(3, 14, "2e04e6bf58cfcbda"), (3, 15, "b868d02f9a568f4b")]),
    ("re._compiler._combine_flags", &[(3, 14, "a7ff10727ef28a64"), (3, 15, "a8fc5ffd51838b75")]),
    ("re._parser.parse", &[(3, 14, "00c1b7bfeea1324c"), (3, 15, "140b242eaf6ba525")]),
    ("re._parser.fix_flags", &[(3, 14, "79a86746d369fb4d"), (3, 15, "cf536644aa362320")]),
    ("re._parser._parse_sub", &[(3, 14, "4f635cd10fde1346"), (3, 15, "2648730590fef8e1")]),
    ("re._parser._parse", &[(3, 14, "e5e3cd0d55da7193"), (3, 15, "aa2ecdd42bfc8f61")]),
    ("re._parser._parse_flags", &[(3, 14, "20cf4973e6c27fb4"), (3, 15, "b4bf7751f9150257")]),
    ("re._parser._escape", &[(3, 14, "5e872535dad3ef18"), (3, 15, "dbb793bdb742ec3c")]),
    ("re._parser._class_escape", &[(3, 14, "ac571e89293380c0"), (3, 15, "5c7ab5562ea230eb")]),
    ("re._parser._uniq", &[(3, 14, "4e83252bbc349edc"), (3, 15, "63f117e6f70703ed")]),
    ("re._parser.parse_template", &[(3, 14, "3a812fec0fabdb59"), (3, 15, "fe2077ba055c234f")]),
    ("re._parser.Tokenizer.__init__", &[(3, 14, "73d276ff15d171a2"), (3, 15, "5736c0c35d265fce")]),
    ("re._parser.Tokenizer.__next", &[(3, 14, "ef0bde27c4849e94"), (3, 15, "299bd916531867d3")]),
    ("re._parser.Tokenizer.match", &[(3, 14, "f1440f32f4e2aaa5"), (3, 15, "aeeede2615c4269a")]),
    ("re._parser.Tokenizer.get", &[(3, 14, "01ffefddc39458cc"), (3, 15, "b34eb37373bf67bb")]),
    ("re._parser.Tokenizer.getwhile", &[(3, 14, "98812dba0f7edfe7"), (3, 15, "f46e01f10e857cb3")]),
    ("re._parser.Tokenizer.getuntil", &[(3, 14, "2d9ba7b2c7a2f1f3"), (3, 15, "fd45b18386989496")]),
    ("re._parser.Tokenizer.pos", &[(3, 14, "3a834b4c13e6fa3b"), (3, 15, "1ed9887aaaabc716")]),
    ("re._parser.Tokenizer.tell", &[(3, 14, "3a834b4c13e6fa3b"), (3, 15, "1ed9887aaaabc716")]),
    ("re._parser.Tokenizer.seek", &[(3, 14, "69bccd8deea19890"), (3, 15, "fb463b9bcc286b6d")]),
    ("re._parser.Tokenizer.checkgroupname", &[(3, 14, "d9011303395c3793"), (3, 15, "7c80f5f37204a35e")]),
    ("re._parser.State.__init__", &[(3, 14, "f0e8ac752d52d3fc"), (3, 15, "195da0acd55075d6")]),
    ("re._parser.State.groups", &[(3, 14, "1635c869aa6581d0"), (3, 15, "32022b1b92df5f05")]),
    ("re._parser.State.opengroup", &[(3, 14, "907b5da2ee3fca19"), (3, 15, "1ddd8187d0400dcc")]),
    ("re._parser.State.closegroup", &[(3, 14, "e1f844548840ff5e"), (3, 15, "453bc7bda2380358")]),
    ("re._parser.State.checkgroup", &[(3, 14, "bfeb0cec11260ad4"), (3, 15, "2d2be7702af5d88e")]),
    ("re._parser.State.checklookbehindgroup", &[(3, 14, "86734079cdd73fbd"), (3, 15, "89904035c6df133d")]),
    ("re._parser.SubPattern.__init__", &[(3, 14, "064404de9d4c9f3a"), (3, 15, "106e3dd1fadcda89")]),
    ("re._parser.SubPattern.__len__", &[(3, 14, "d8c3eff686852e98"), (3, 15, "a6b3b9620b8fc052")]),
    ("re._parser.SubPattern.__delitem__", &[(3, 14, "211edeb3d594ec8d"), (3, 15, "0824fc48e76dffb2")]),
    ("re._parser.SubPattern.__getitem__", &[(3, 14, "e3bf60cafcf29041"), (3, 15, "45c6a3ac6b3a6031")]),
    ("re._parser.SubPattern.__setitem__", &[(3, 14, "4f6ad60476be842c"), (3, 15, "b10a8e2a1402fcfc")]),
    ("re._parser.SubPattern.insert", &[(3, 14, "fc1ab858279ebc1b"), (3, 15, "5cb4e4f263e3e937")]),
    ("re._parser.SubPattern.append", &[(3, 14, "24fa4030f6f224fe"), (3, 15, "c3b8b24b4f003431")]),
    ("re._parser.SubPattern.getwidth", &[(3, 14, "8f37cde82f20e669"), (3, 15, "cc8ae602ba117bac")]),
    ("re.match", &[(3, 14, "f5a31e6ddd1c26f0"), (3, 15, "13cc452e7e2ac341")]),
    ("re.fullmatch", &[(3, 14, "98f6ca419897894d"), (3, 15, "0f884cde6604c745")]),
    ("re.search", &[(3, 14, "b3ab9db415bd2b35"), (3, 15, "36f515ce77ea527e")]),
    ("re.sub", &[(3, 14, "2139feca00ad0926"), (3, 15, "980bcb970b910f2b")]),
    ("re.subn", &[(3, 14, "179ee624a31bbffb"), (3, 15, "7424a87aa2248e89")]),
    ("re.split", &[(3, 14, "6eb6bb2dc3bd65de"), (3, 15, "1fd1d1f1335cd34e")]),
    ("re.findall", &[(3, 14, "f9f3bdb8dcb82024"), (3, 15, "6f9912fee4ee0864")]),
    ("re.finditer", &[(3, 14, "4c3cbcd0694c13e4"), (3, 15, "56ff512d368c3f63")]),
    ("re.compile", &[(3, 14, "a2ebcd8a548a953a"), (3, 15, "28f9681cad925e2b")]),
    ("re._compile", &[(3, 14, "638524c6d0b543cc"), (3, 15, "da7d49c39bd21137")]),
    ("re._compile_template", &[(3, 14, "1d749fe0c188c2af"), (3, 15, "c6fb81de71ea8209")]),
];

/// Content hash of `re._casefix._EXTRA_CASES` (FNV-1a 64 over the little-endian u32 words
/// `k, len(v), *v` of the sorted items; the Unicode database's case exceptions): the same
/// table on 3.14 and 3.15.
pub(crate) const EXTRA_CASES: Fingerprints = &[(3, 14, "f828c54725a0307d"), (3, 15, "f828c54725a0307d")];
