#!/usr/bin/env python3
"""Verify reviewed upstream additions and retained terms through the portable engine."""
import ctypes
import importlib.util
import re
import subprocess
import sys
import tempfile
from pathlib import Path

root=Path(__file__).resolve().parents[2]
resources=Path(sys.argv[1]) if len(sys.argv)>1 else root/'.build/Meowpuut.app/Contents/Resources'
sys.path.insert(0,str(root/'scripts'))
spec=importlib.util.spec_from_file_location('compiler',root/'scripts/compile-native-lexicon.py')
compiler=importlib.util.module_from_spec(spec);spec.loader.exec_module(compiler)
cases=[
 ('二次元','二次元','er4 ci4 yuan2'),('動態島','动态岛','dong4 tai4 dao3'),
 ('截圖','截图','jie2 tu2'),('限時動態','限时动态','xian4 shi2 dong4 tai4'),
 ('群聊','群聊','qun2 liao2'),('頂不住','顶不住','ding3 bu2 zhu4'),
 ('帖子','帖子','tie3 zi5'),('走起','走起','zou3 qi3'),('高亮','高亮','gao1 liang4'),
 ('調用','调用','diao4 yong4'),('虛擬機','虚拟机','xu1 ni3 ji1'),
 ('正則表達式','正则表达式','zheng4 ze2 biao3 da2 shi4'),
]
with tempfile.TemporaryDirectory(prefix='mimo-upstream-') as tmp:
 lib=Path(tmp)/('core.dylib' if sys.platform=='darwin' else 'core.so')
 flags=['-dynamiclib'] if sys.platform=='darwin' else ['-shared','-fPIC']
 subprocess.run(['c++','-std=c++17','-O2',*flags,str(root/'Core/MeowpuutCore.cpp'),'-o',str(lib)],check=True)
 c=ctypes.CDLL(str(lib))
 for name,args,restype in [
  ('mpn_lexicon_create',[ctypes.c_char_p,ctypes.c_size_t],ctypes.c_void_p),('mpn_lexicon_destroy',[ctypes.c_void_p],None),
  ('mpn_session_create',[ctypes.c_void_p],ctypes.c_void_p),('mpn_session_destroy',[ctypes.c_void_p],None),
  ('mpn_set_simplified',[ctypes.c_void_p,ctypes.c_int],None),('mpn_clear',[ctypes.c_void_p],None),
  ('mpn_key',[ctypes.c_void_p,ctypes.c_int32],ctypes.c_int),('mpn_candidate_count',[ctypes.c_void_p],ctypes.c_size_t),
  ('mpn_candidate',[ctypes.c_void_p,ctypes.c_size_t],ctypes.c_char_p),('mpn_choose',[ctypes.c_void_p,ctypes.c_size_t],ctypes.c_int),
  ('mpn_committed',[ctypes.c_void_p],ctypes.c_char_p)]:
  f=getattr(c,name);f.argtypes=args;f.restype=restype
 for mode in ('pinyin','zhuyin'):
  data=(resources/f'native-{mode}.tsv').read_bytes();lex=c.mpn_lexicon_create(data,len(data));assert lex
  ses=c.mpn_session_create(lex);assert ses
  try:
   for hant,hans,reading in cases:
    if mode=='pinyin':raw=reading.replace(' ','');plain=re.sub('[1-5]','',raw)
    else:raw=''.join(compiler.encode(x) for x in reading.split());plain=re.sub('[ 6347]','',raw)
    for keys in (raw,plain):
     for simplified in (0,1):
      c.mpn_clear(ses);c.mpn_set_simplified(ses,simplified)
      for k in keys:assert c.mpn_key(ses,ord(k)),(mode,keys,k)
      choices=[c.mpn_candidate(ses,i).decode() for i in range(c.mpn_candidate_count(ses))]
      target=(hant,hans)[simplified]
      assert target in choices[:7],(mode,keys,target,choices[:10])
      assert c.mpn_choose(ses,choices.index(target)) and c.mpn_committed(ses).decode()==target
  finally:c.mpn_session_destroy(ses);c.mpn_lexicon_destroy(lex)
print('PASS: upstream additions and retained terms, both modes/scripts/tones and first-page selection')
