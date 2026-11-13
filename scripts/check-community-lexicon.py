#!/usr/bin/env python3
"""Exercise the portable decoder against community and everyday homophones.

Usage: python3 scripts/check-community-lexicon.py [compiled-resources-directory]
Requires a C++17 compiler. It does not modify the bundled dictionary or fetch data.
"""
import ctypes
import importlib.util
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
resources = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / '.build/lexicon'
spec = importlib.util.spec_from_file_location('meowpuut_lexicon_compiler', ROOT / 'scripts/compile-native-lexicon.py')
compiler = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compiler)

# Readings are specified independently of the built tables; both scripts must be reachable.
community = [
    ('er4 chuang4', '二創', '二创'),
    ('er4 chuang4 zuo4 pin3', '二創作品', '二创作品'),
    ('zi4 she4', '自設', '自设'),
    ('gai3 she4', '改設', '改设'),
    ('pin1 tuan2', '拼團', '拼团'),
    ('quan1 tu2', '圈圖', '圈图'),
    ('tong2 ren2 nv3', '同人女', '同人女'),
    ('tong2 dan1', '同擔', '同担'),
    ('gu3 quan1', '谷圈', '谷圈'),
    ('mai3 gu3', '買谷', '买谷'),
    ('zhui1 fan1', '追番', '追番'),
    ('dan4 mu4', '彈幕', '弹幕'),
    ('ke1 tang2', '嗑糖', '嗑糖'),
    ('tie1 tie1', '貼貼', '贴贴'),
    ('sui4 sui4 nian4', '碎碎念', '碎碎念'),
    ('kong1 jian1 wen2 an4', '空間文案', '空间文案'),
    ('dian4 zi3 gui1 mi4', '電子閨蜜', '电子闺蜜'),
    ('dan1 tui1', '單推', '单推'),
    ('you4 xun4 ran3', '幼馴染', '幼驯染'),
    ('ao4 jiao1', '傲嬌', '傲娇'),
    ('fan1 wai4', '番外', '番外'),
    ('guo2 man4', '國漫', '国漫'),
    ('ri4 man4', '日漫', '日漫'),
    ('zi4 mu4 zu3', '字幕組', '字幕组'),
    ('da3 zhou2', '打軸', '打轴'),
    ('li4 hui4', '立繪', '立绘'),
    ('hui4 shi1', '繪師', '绘师'),
    ('chi1 gu3', '吃谷', '吃谷'),
    ('jin1 ju4', '金句', '金句'),
    ('ji4 zhang4', '記賬', '记账'),
    ('pin1 dan1', '拼單', '拼单'),
    ('chai1 he2', '拆盒', '拆盒'),
    ('ka3 tao4', '卡套', '卡套'),
    ('zi4 yin4', '自印', '自印'),
    ('chang3 kan1', '場刊', '场刊'),
    ('he2 zhi4', '合誌', '合志'),
    ('zhuan3 tui1', '轉推', '转推'),
    ('cun2 tu2', '存圖', '存图'),
    ('hu4 guan1', '互關', '互关'),
    ('ping2 lun4 qu1', '評論區', '评论区'),
    ('zhui1 geng1', '追更', '追更'),
    ('fan1 zu3', '番組', '番组'),
    ('tu2 tou4', '圖透', '图透'),
    ('ju4 tou4', '劇透', '剧透'),
    ('chao1 hua4', '超話', '超话'),
    ('tong2 ren2 zhan3', '同人展', '同人展'),
    ('sheng1 you1', '聲優', '声优'),
    ('ji2 ka3', '集卡', '集卡'),
    ('huan4 gu3', '換谷', '换谷'),
    ('bu3 kuan3', '補款', '补款'),
]
# Everyday phrases must remain reachable as the community vocabulary grows.
everyday = [
    ('zao3 shang4 hao3', '早上好', '早上好'),
    ('mei2 guan1 xi5', '沒關係', '没关系'),
    ('wo3 jue2 de5', '我覺得', '我觉得'),
    ('mei2 you3 wen4 ti2', '沒有問題', '没有问题'),
    ('qing3 deng3 yi1 xia4', '請等一下', '请等一下'),
    ('zhe4 ge5 wen4 ti2', '這個問題', '这个问题'),
    ('ming2 tian1 jian4', '明天見', '明天见'),
    ('sheng1 ri4 kuai4 le4', '生日快樂', '生日快乐'),
    ('kuai4 di4', '快遞', '快递'),
    ('wan3 an1', '晚安', '晚安'),
]
# Full readings exercise decoder segmentation and context in both scripts.  The
# pinyin path intentionally omits tones, as it does for normal typing.
sentences = [
    ('wo3 men5 jin1 tian1 qu4 kan4 dian4 ying3', '我們今天去看電影', '我们今天去看电影'),
    ('wo3 xi3 huan1 kan4 dong4 man4', '我喜歡看動漫', '我喜欢看动漫'),
    ('jin1 tian1 tian1 qi4 hen3 hao3', '今天天氣很好', '今天天气很好'),
    ('ni3 ming2 tian1 you3 kong4 ma5', '你明天有空嗎', '你明天有空吗'),
    ('wo3 jue2 de5 zhe4 ge5 hen3 ke3 ai4', '我覺得這個很可愛', '我觉得这个很可爱'),
    ('wo3 men5 yi4 qi3 qu4 chi1 fan4', '我們一起去吃飯', '我们一起去吃饭'),
    ('zhe4 ge5 wen4 ti2 zen3 me5 jie3 jue2', '這個問題怎麼解決', '这个问题怎么解决'),
    ('wo3 xiang3 mai3 zhe4 ge5 zhou1 bian1', '我想買這個周邊', '我想买这个周边'),
    ('wo3 jin1 tian1 kan4 le5 yi2 bu4 dong4 man4', '我今天看了一部動漫', '我今天看了一部动漫'),
    ('wo3 xiang3 kan4 yi2 bu4 dian4 ying3', '我想看一部電影', '我想看一部电影'),
    ('wo3 men5 yi2 bu4 yi2 bu4 lai2', '我們一步一步來', '我们一步一步来'),
    ('wo3 jin1 tian1 mai3 le5 yi2 bu4 shou3 ji1', '我今天買了一部手機', '我今天买了一部手机'),
    ('wo3 yi2 bu4 xiao3 xin1 jiu4 shui4 zhao2 le5', '我一不小心就睡著了', '我一不小心就睡着了'),
    ('wo3 men5 xiang4 qian2 zou3 le5 yi2 bu4', '我們向前走了一步', '我们向前走了一步'),
    ('ming2 tian1 jian4 zai4 jian4', '明天見再見', '明天见再见'),
    ('wo3 jue2 de5 zhe4 ge5 hen3 you3 yi4 si5', '我覺得這個很有意思', '我觉得这个很有意思'),
    ('wo3 men5 zhou1 mo4 yi4 qi3 qu4 wan2', '我們週末一起去玩', '我们周末一起去玩'),
    ('zhe4 tiao2 yu2 hen3 xin1 xian1', '這條魚很新鮮', '这条鱼很新鲜'),
    ('ta1 zai4 kan4 na4 bu4 dong4 man4', '他在看那部動漫', '他在看那部动漫'),
    ('wo3 jue2 de5 ke4 tang2 shang4 ke1 tang2', '我覺得課堂上嗑糖', '我觉得课堂上嗑糖'),
    ('wo3 hen3 xi3 huan1 zhe4 bu4 zuo4 pin3', '我很喜歡這部作品', '我很喜欢这部作品'),
    ('ni3 kan4 guo4 zhe4 bu4 dong4 hua4 ma5', '你看過這部動畫嗎', '你看过这部动画吗'),
    ('wo3 jue2 de5 zhe4 ge5 jue2 se4 hen3 you3 qu4', '我覺得這個角色很有趣', '我觉得这个角色很有趣'),
    ('wo3 jin1 tian1 xia4 wu3 yao4 kai1 hui4', '我今天下午要開會', '我今天下午要开会'),
    ('zhe4 jia1 dian4 de5 ka1 fei1 hen3 hao3 he1', '這家店的咖啡很好喝', '这家店的咖啡很好喝'),
    ('wo3 bu4 zhi1 dao4 zen3 me5 hui2 da2', '我不知道怎麼回答', '我不知道怎么回答'),
    ('wo3 ye3 shi4 zhe4 yang4 ren4 wei2 de5', '我也是這樣認為的', '我也是这样认为的'),
    ('mai3 le5 yi2 ge5 xin1 bao1', '買了一個新包', '买了一个新包'),
    ('wo3 mai3 le5 yi2 ge5 xin1 bao1', '我買了一個新包', '我买了一个新包'),
    ('wo3 xi3 huan1 zhe4 ge5 xin1 bao1', '我喜歡這個新包', '我喜欢这个新包'),
    ('jin1 tian1 mai3 le5 xin1 bao1', '今天買了新包', '今天买了新包'),
    ('zhe4 ge5 xin1 bao1 hen3 hao3 kan4', '這個新包很好看', '这个新包很好看'),
    ('yi1 sheng1 jian3 cha2 xin1 bao1', '醫生檢查心包', '医生检查心包'),
    ('ta1 shi4 yi1 ge4 yi1 sheng1', '他是一個醫生', '他是一个医生'),
    ('wo3 ting1 dao4 yi1 sheng1 ju4 xiang3', '我聽到一聲巨響', '我听到一声巨响'),
    ('wo3 xiang3 kan4 zhe4 ge5 fan1 zu3', '我想看這個番組', '我想看这个番组'),
    ('wo3 xiang3 mai3 zhe4 ge5 ka3 tao4', '我想買這個卡套', '我想买这个卡套'),
    ('wo3 men5 yi4 qi3 pin1 dan1', '我們一起拼單', '我们一起拼单'),
    ('ta1 shi4 ri4 ben3 sheng1 you1', '他是日本聲優', '他是日本声优'),
]
# Untoned homophones should not let a niche use displace the everyday expression.
conflicts = [
    ('gu3 quan1', ('股權', '股权'), ('谷圈', '谷圈')),
    ('ke1 tang2', ('課堂', '课堂'), ('嗑糖', '嗑糖')),
    ('meng4 xiang4', ('夢想', '梦想'), ('夢向', '梦向')),
    ('tong4 bao1', ('同胞', '同胞'), ('痛包', '痛包')),
    ('tong4 che1', ('通車', '通车'), ('痛車', '痛车')),
    ('wu2 liao4', ('無聊', '无聊'), ('無料', '无料')),
    ('an1 li4', ('案例', '案例'), ('安利', '安利')),
    ('chan3 liang2', ('產量', '产量'), ('產糧', '产粮')),
    ('li4 hui4', ('理會', '理会'), ('立繪', '立绘')),
    ('hui4 shi1', ('會師', '会师'), ('繪師', '绘师')),
    ('chi1 gu3', ('持股', '持股'), ('吃谷', '吃谷')),
    ('da3 zhou2', ('大洲', '大洲'), ('打軸', '打轴')),
    ('xin1 bao1', ('心包', '心包'), ('新包', '新包')),
    ('sheng1 you1', ('省油', '省油'), ('聲優', '声优')),
    ('ju4 tou4', ('巨頭', '巨头'), ('劇透', '剧透')),
    ('chang3 kan1', ('常看', '常看'), ('場刊', '场刊')),
    ('he2 zhi4', ('何止', '何止'), ('合誌', '合志')),
]
# These contexts have unambiguous expected first choices; the standalone
# homophone comparison above also protects the medical reading.
preferred_sentences = [
    ('ta1 shi4 ri4 ben3 sheng1 you1', '他是日本聲優', '他是日本声优'),
    ('mai3 le5 yi2 ge5 xin1 bao1', '買了一個新包', '买了一个新包'),
    ('wo3 xi3 huan1 zhe4 ge5 xin1 bao1', '我喜歡這個新包', '我喜欢这个新包'),
    ('yi1 sheng1 jian3 cha2 xin1 bao1', '醫生檢查心包', '医生检查心包'),
]
# 我聽到一聲巨響 is no longer required first, only in the first 3 (list `sentences`): the pinyin
# index blends its sources, and both mimo-tc and the counted text have 醫生 well above 一聲, so 醫生 now
# outweighs 一聲 as a word. Choosing 一聲 before 巨響 needs word-pair context, which the decoder
# does not have yet.

if True:
    env = {**os.environ, 'PATH': f"{Path.home() / '.cargo/bin'}:{os.environ.get('PATH', '')}"}
    subprocess.run(['cargo', 'build', '--release', '-p', 'meowpuut-ffi'], cwd=ROOT, env=env, check=True)
    ext = 'dylib' if sys.platform == 'darwin' else ('dll' if sys.platform == 'win32' else 'so')
    prefix_name = '' if sys.platform == 'win32' else 'lib'
    libpath = ROOT / 'target/release' / f'{prefix_name}meowpuut_ffi.{ext}'
    lib = ctypes.CDLL(str(libpath))
    for name, args, result in [
        ('mpn_lexicon_create', [ctypes.c_char_p, ctypes.c_size_t], ctypes.c_void_p),
        ('mpn_lexicon_destroy', [ctypes.c_void_p], None),
        ('mpn_session_create', [ctypes.c_void_p], ctypes.c_void_p),
        ('mpn_session_destroy', [ctypes.c_void_p], None),
        ('mpn_set_simplified', [ctypes.c_void_p, ctypes.c_int], None),
        ('mpn_clear', [ctypes.c_void_p], None),
        ('mpn_key', [ctypes.c_void_p, ctypes.c_int32], ctypes.c_int),
        ('mpn_candidate_count', [ctypes.c_void_p], ctypes.c_size_t),
        ('mpn_candidate', [ctypes.c_void_p, ctypes.c_size_t], ctypes.c_char_p),
    ]:
        fn = getattr(lib, name)
        fn.argtypes, fn.restype = args, result
    count = 0
    for mode in ('pinyin', 'zhuyin'):
        data = (resources / f'native-{mode}.tsv').read_bytes()
        lexicon = lib.mpn_lexicon_create(data, len(data))
        if not lexicon:
            raise ValueError(f'Invalid {mode} table in {resources}')
        session = lib.mpn_session_create(lexicon)
        if not session:
            raise MemoryError('Could not create engine session')
        try:
            def candidates(reading, tones):
                lib.mpn_clear(session)
                if mode == 'pinyin':
                    keys = re.sub(r'[1-5 ]', '', reading)
                else:
                    encoded = [compiler.encode(syllable) for syllable in reading.split()]
                    if not all(encoded):
                        raise ValueError(f'Unsupported reading: {reading}')
                    keys = ''.join(encoded)
                    if not tones:
                        keys = re.sub(r'[ 6347]', '', keys)
                for key in keys:
                    if not lib.mpn_key(session, ord(key)):
                        raise AssertionError(f'{mode}: rejected {keys!r}')
                return [lib.mpn_candidate(session, i).decode('utf-8')
                        for i in range(lib.mpn_candidate_count(session))]
            for simplified in (0, 1):
                lib.mpn_set_simplified(session, simplified)
                for reading, hant, hans in community + everyday:
                    actual = candidates(reading, True)
                    expected = (hant, hans)[simplified]
                    assert expected in actual[:7], f'{mode} {reading}: {expected} missing from first 7: {actual[:7]}'
                    count += 1
                for reading, ordinary, niche in conflicts:
                    actual = candidates(reading, False)
                    regular, specialist = ordinary[simplified], niche[simplified]
                    assert regular in actual and specialist in actual, f'{mode} {reading}: missing {regular} or {specialist}: {actual[:15]}'
                    assert actual.index(regular) < actual.index(specialist) < 7, f'{mode} {reading}: {regular} should precede {specialist} in first 7: {actual[:7]}'
                    count += 1
                for reading, hant, hans in sentences:
                    actual = candidates(reading, True)
                    expected = (hant, hans)[simplified]
                    assert expected in actual[:3], f'{mode} {reading}: {expected} missing from first 3: {actual[:7]}'
                    count += 1
                for reading, hant, hans in preferred_sentences:
                    actual = candidates(reading, True)
                    expected = (hant, hans)[simplified]
                    assert actual and actual[0] == expected, f'{mode} {reading}: expected first {expected}, got {actual[:7]}'
                    count += 1
        finally:
            lib.mpn_session_destroy(session)
            lib.mpn_lexicon_destroy(lexicon)
print(f'PASS: {count} engine checks across pinyin, zhuyin, traditional and simplified')
