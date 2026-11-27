/*
 * macOS Voice Audio, DSP SpeechGate/Normalizer/WAV, Cue Player, AVAudioEngine Recorder,
 * SFSpeechRecognizer, Cloud/Local HTTP & CLI Pipeline, Resident whisper-server & Qwen3-ASR Helper,
 * and Local Model Installer (1:1 port of VoiceEngine, VoiceWhisperServer, VoiceQwenHelper,
 * VoiceASREngine, VoiceLocalInstaller, and VoiceCuePlayer).
 */

#import <Accelerate/Accelerate.h>
#import <AppKit/AppKit.h>
#import <AVFoundation/AVFoundation.h>
#import <CommonCrypto/CommonDigest.h>
#import <Foundation/Foundation.h>
#import <Speech/Speech.h>
#include <arpa/inet.h>
#include <mach/mach_time.h>
#include <math.h>
#include <netinet/in.h>
#include <signal.h>
#include <sys/socket.h>

extern void gc_handle_free(int64_t handle);
typedef void (*juxu_voice_cb0_t)(int64_t handle);
typedef void (*juxu_voice_cb_str_t)(int64_t handle, void *nsStr);
typedef void (*juxu_voice_cb_ok_str_t)(int64_t handle, bool ok, void *nsStr);
typedef void (*juxu_voice_cb_prog_t)(int64_t handle, int32_t kind, double progress, void *nsName, void *nsDetail);
typedef void (*juxu_voice_cb_level_t)(int64_t handle, double level, void *nsPartial);

static NSString *jv_ns(void *p) { return p ? (__bridge NSString *)p : @""; }
static void *jv_ret_ns(NSString *s) {
    if (!s || s.length == 0) return (__bridge void *)@"";
    NSString *cp = [s copy];
    return (void *)CFAutorelease(CFBridgingRetain(cp));
}

/* ---- 1. Hant -> Hans Simplified Conversion (`VoiceWhisperStreamPolicy.simplified`) ---- */

void *juxu_voice_simplified_ns(void *nsText, void *nsLang) {
    NSString *text = jv_ns(nsText);
    NSString *lang = [jv_ns(nsLang) lowercaseString];
    if (![lang hasPrefix:@"zh"] || text.length == 0) return jv_ret_ns(text);
    static NSCharacterSet *keepSet = nil;
    static dispatch_once_t onceToken;
    dispatch_once(&onceToken, ^{
        keepSet = [NSCharacterSet characterSetWithCharactersInString:@"藉俱乾著阪苎吒姊箝麴跤摺瞭"];
    });
    NSMutableString *out = [NSMutableString stringWithCapacity:text.length];
    NSMutableString *run = [NSMutableString string];
    void (^flush)(void) = ^{
        if (run.length == 0) return;
        NSMutableString *tmp = [run mutableCopy];
        CFStringTransform((__bridge CFMutableStringRef)tmp, NULL, CFSTR("Hant-Hans"), false);
        [out appendString:tmp];
        [run setString:@""];
    };
    for (NSUInteger i = 0; i < text.length; i++) {
        unichar c = [text characterAtIndex:i];
        if ([keepSet characterIsMember:c]) {
            flush();
            [out appendFormat:@"%C", c];
        } else {
            [run appendFormat:@"%C", c];
        }
    }
    flush();
    return jv_ret_ns(out);
}

/* ---- 2. Synthesized Start / End Cue Player (`VoiceCuePlayer`) ---- */

static NSSound *jv_make_cue_sound(double fromHz, double toHz) {
    const double rate = 44100.0;
    const double secs = 0.07;
    const int count = (int)(rate * secs);
    uint32_t byteCount = (uint32_t)(count * 2);
    NSMutableData *wav = [NSMutableData dataWithCapacity:44 + byteCount];
    uint32_t riffSz = CFSwapInt32HostToLittle(36 + byteCount);
    uint32_t fmtSz = CFSwapInt32HostToLittle(16);
    uint16_t pcm1 = CFSwapInt16HostToLittle(1);
    uint32_t uRate = CFSwapInt32HostToLittle(44100);
    uint32_t bRate = CFSwapInt32HostToLittle(88200);
    uint16_t align = CFSwapInt16HostToLittle(2);
    uint16_t bits = CFSwapInt16HostToLittle(16);
    uint32_t dataSz = CFSwapInt32HostToLittle(byteCount);
    [wav appendBytes:"RIFF" length:4]; [wav appendBytes:&riffSz length:4];
    [wav appendBytes:"WAVEfmt " length:8]; [wav appendBytes:&fmtSz length:4];
    [wav appendBytes:&pcm1 length:2]; [wav appendBytes:&pcm1 length:2];
    [wav appendBytes:&uRate length:4]; [wav appendBytes:&bRate length:4];
    [wav appendBytes:&align length:2]; [wav appendBytes:&bits length:2];
    [wav appendBytes:"data" length:4]; [wav appendBytes:&dataSz length:4];
    double phase = 0.0;
    for (int i = 0; i < count; i++) {
        double t = (double)i / (double)count;
        phase += 2.0 * M_PI * (fromHz + (toHz - fromHz) * t) / rate;
        double env = fmin(1.0, t / 0.08) * (1.0 - t) * (1.0 - t);
        int16_t s = (int16_t)CFSwapInt16HostToLittle((uint16_t)(int16_t)(sin(phase) * env * 0.8 * 32767.0));
        [wav appendBytes:&s length:2];
    }
    return [[NSSound alloc] initWithData:wav];
}

void juxu_voice_play_cue(int64_t kind) {
    static NSSound *sStart = nil;
    static NSSound *sEnd = nil;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        sStart = jv_make_cue_sound(660.0, 880.0);
        sEnd = jv_make_cue_sound(880.0, 620.0);
    });
    NSSound *snd = (kind == 0) ? sStart : sEnd;
    if (!snd) return;
    [snd stop];
    snd.volume = 0.14f;
    [snd play];
}

/* ---- 3. SpeechGate + AudioNormalizer + WAV Encoder (`VoiceSpeechGate`, `VoiceAudioNormalizer`) ---- */

static double jv_voiced_seconds(const int16_t *samples, size_t nSamples, bool trustNoiseFloor, int *outFirstFrame) {
    const int hop = 320, win = 640, minLag = 40, maxLag = 228;
    if (outFirstFrame) *outFirstFrame = -1;
    if (!samples || nSamples < (size_t)win) return 0.0;

    static float hann[640];
    static float hannAc[229];
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        for (int i = 0; i < win; i++) {
            hann[i] = (float)(0.5 - 0.5 * cos(2.0 * M_PI * (double)i / (double)(win - 1)));
        }
        float zeroSum = 1.0f;
        for (int lag = 0; lag <= maxLag; lag++) {
            float sum = 0.0f;
            vDSP_dotpr(hann, 1, hann + lag, 1, &sum, (vDSP_Length)(win - lag));
            if (lag == 0) zeroSum = fmaxf(sum, 1e-6f);
            hannAc[lag] = fmaxf(sum / zeroSum, 1e-6f);
        }
    });

    int fCount = (int)((nSamples - (size_t)win) / (size_t)hop + 1);
    float *levels = (float *)calloc((size_t)fCount, sizeof(float));
    float *voicing = (float *)calloc((size_t)fCount, sizeof(float));
    float raw[640], x[640];
    int unvoicedCnt = 0;
    for (int f = 0; f < fCount; f++) {
        size_t st = (size_t)f * (size_t)hop;
        for (int i = 0; i < win; i++) raw[i] = (float)samples[st + (size_t)i];
        float pwr = 0.0f, mean = 0.0f, energy = 0.0f;
        vDSP_svesq(raw, 1, &pwr, (vDSP_Length)win);
        levels[f] = sqrtf(pwr / (float)win);
        vDSP_meanv(raw, 1, &mean, (vDSP_Length)win);
        for (int i = 0; i < win; i++) x[i] = (raw[i] - mean) * hann[i];
        vDSP_svesq(x, 1, &energy, (vDSP_Length)win);
        if (energy > 0.0f) {
            float best = 0.0f;
            for (int lag = minLag; lag <= maxLag; lag++) {
                float r = 0.0f;
                vDSP_dotpr(x, 1, x + lag, 1, &r, (vDSP_Length)(win - lag));
                float v = (r / energy) / hannAc[lag];
                if (v > best) best = v;
            }
            voicing[f] = fminf(best, 1.2f);
        }
        if (voicing[f] < 0.32f) unvoicedCnt++;
    }

    bool reliable = trustNoiseFloor && unvoicedCnt >= (fCount / 10 > 5 ? fCount / 10 : 5);
    int srcCnt = reliable ? unvoicedCnt : fCount;
    float *sorted = (float *)malloc((size_t)srcCnt * sizeof(float));
    int si = 0;
    for (int f = 0; f < fCount; f++) {
        if (!reliable || voicing[f] < 0.32f) sorted[si++] = levels[f];
    }
    vDSP_vsort(sorted, (vDSP_Length)srcCnt, 1);
    float pos = (float)(srcCnt - 1) * 0.1f;
    int lo = (int)pos;
    int hi = (lo + 1 < srcCnt) ? lo + 1 : srcCnt - 1;
    float floorVal = sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - (float)lo);
    free(sorted);
    float floorThr = floorVal * 1.20f;
    float thr = fmaxf(60.0f, reliable ? floorThr : fminf(floorThr, 200.0f));

    int *vIdx = (int *)malloc((size_t)fCount * sizeof(int));
    int vCnt = 0;
    for (int f = 0; f < fCount; f++) {
        if (voicing[f] >= 0.32f && levels[f] >= thr) vIdx[vCnt++] = f;
    }

    bool *flags = (bool *)calloc((size_t)fCount, sizeof(bool));
    if (vCnt >= 5) {
        int runStart = 0;
        for (int i = 0; i < vCnt; i++) {
            bool ends = (i == vCnt - 1) || (vIdx[i + 1] - vIdx[i] > 11);
            if (!ends) continue;
            int rLen = i - runStart + 1;
            int span = vIdx[i] - vIdx[runStart] + 1;
            if ((double)rLen / (double)span >= 0.5) {
                bool speech = (rLen >= 12);
                if (!speech && rLen >= 5) {
                    float *rl = (float *)malloc((size_t)rLen * sizeof(float));
                    for (int k = 0; k < rLen; k++) rl[k] = levels[vIdx[runStart + k]];
                    vDSP_vsort(rl, (vDSP_Length)rLen, 1);
                    float med = (rLen % 2 == 0) ? 0.5f * (rl[rLen / 2 - 1] + rl[rLen / 2]) : rl[rLen / 2];
                    free(rl);
                    speech = (med >= 80.0f);
                }
                if (speech) {
                    for (int k = runStart; k <= i; k++) flags[vIdx[k]] = true;
                }
            }
            runStart = i + 1;
        }
    }

    int totalVoiced = 0;
    for (int f = 0; f < fCount; f++) {
        if (flags[f]) {
            if (outFirstFrame && *outFirstFrame < 0) *outFirstFrame = f;
            totalVoiced++;
        }
    }
    free(flags); free(vIdx); free(voicing); free(levels);
    return (double)totalVoiced * (double)hop / 16000.0;
}

static NSData *jv_condition_and_encode_wav(const int16_t *samples, size_t count) {
    if (!samples || count == 0) return [NSData data];
    float *flt = (float *)malloc(count * sizeof(float));
    float prevIn = 0.0f, prevOut = 0.0f, peak = 0.0f, sumSq = 0.0f;
    const float inv = 1.0f / 32768.0f;
    for (size_t i = 0; i < count; i++) {
        float x = (float)samples[i] * inv;
        float y = x - prevIn + 0.969f * prevOut;
        prevIn = x; prevOut = y;
        flt[i] = y;
        float a = fabsf(y);
        if (a > peak) peak = a;
        sumSq += y * y;
    }
    float gain = 1.0f;
    if (peak >= 0.004f && peak < 0.70f) {
        float rms = sqrtf(sumSq / (float)count);
        float pGain = 0.70f / peak;
        float rGain = rms > 1e-5f ? (0.11f / rms) * 1.6f : pGain;
        gain = fminf(8.0f, fmaxf(1.0f, fminf(pGain, rGain)));
    }
    uint32_t dataSz = (uint32_t)(count * 2);
    NSMutableData *wav = [NSMutableData dataWithLength:44 + dataSz];
    uint8_t *dst = (uint8_t *)wav.mutableBytes;
    uint32_t riffSz = CFSwapInt32HostToLittle(36 + dataSz);
    uint32_t fmtSz = CFSwapInt32HostToLittle(16);
    uint16_t one = CFSwapInt16HostToLittle(1);
    uint32_t sRate = CFSwapInt32HostToLittle(16000);
    uint32_t bRate = CFSwapInt32HostToLittle(32000);
    uint16_t align = CFSwapInt16HostToLittle(2);
    uint16_t bits = CFSwapInt16HostToLittle(16);
    uint32_t leDataSz = CFSwapInt32HostToLittle(dataSz);
    memcpy(dst + 0, "RIFF", 4); memcpy(dst + 4, &riffSz, 4);
    memcpy(dst + 8, "WAVEfmt ", 8); memcpy(dst + 16, &fmtSz, 4);
    memcpy(dst + 20, &one, 2); memcpy(dst + 22, &one, 2);
    memcpy(dst + 24, &sRate, 4); memcpy(dst + 28, &bRate, 4);
    memcpy(dst + 32, &align, 2); memcpy(dst + 34, &bits, 2);
    memcpy(dst + 36, "data", 4); memcpy(dst + 40, &leDataSz, 4);
    int16_t *pcmOut = (int16_t *)(dst + 44);
    for (size_t i = 0; i < count; i++) {
        float v = flt[i] * gain;
        float a = fabsf(v);
        if (a > 0.88f) {
            float lim = 0.88f + 0.12f * tanhf((a - 0.88f) / 0.12f);
            v = v >= 0.0f ? lim : -lim;
        }
        float sc = fmaxf(-32768.0f, fminf(32767.0f, roundf(v * 32767.0f)));
        pcmOut[i] = (int16_t)CFSwapInt16HostToLittle((uint16_t)(int16_t)sc);
    }
    free(flt);
    return wav;
}

/* Exposed for unit tests in Geisha */
double juxu_voice_test_speech_gate(const int16_t *samples, int64_t count) {
    if (count <= 0) return 0.0;
    return jv_voiced_seconds(samples, (size_t)count, true, NULL);
}

/* ---- 4. Permissions & Local Tool / Model Status (`VoiceLocalInstaller` probes) ---- */

void juxu_voice_request_mic_speech(bool needSpeech, void *fnPtr, int64_t cbHandle) {
    juxu_voice_cb_ok_str_t cb = (juxu_voice_cb_ok_str_t)fnPtr;
    void (^finish)(bool, NSString *) = ^(bool ok, NSString *msg) {
        dispatch_async(dispatch_get_main_queue(), ^{
            if (cb) cb(cbHandle, ok, jv_ret_ns(msg));
            gc_handle_free(cbHandle);
        });
    };
    void (^afterMic)(BOOL) = ^(BOOL micGranted) {
        if (!micGranted) {
            finish(false, @"请在「系统设置 → 隐私与安全性 → 麦克风」中允许句序访问麦克风");
            return;
        }
        if (!needSpeech) {
            finish(true, @"");
            return;
        }
        SFSpeechRecognizerAuthorizationStatus st = [SFSpeechRecognizer authorizationStatus];
        if (st == SFSpeechRecognizerAuthorizationStatusAuthorized) {
            finish(true, @"");
        } else if (st == SFSpeechRecognizerAuthorizationStatusNotDetermined) {
            [SFSpeechRecognizer requestAuthorization:^(SFSpeechRecognizerAuthorizationStatus status) {
                if (status == SFSpeechRecognizerAuthorizationStatusAuthorized) finish(true, @"");
                else finish(false, @"请在「系统设置 → 隐私与安全性 → 语音识别」中允许句序使用语音识别");
            }];
        } else {
            finish(false, @"请在「系统设置 → 隐私与安全性 → 语音识别」中允许句序使用语音识别");
        }
    };
    AVAuthorizationStatus mst = [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
    if (mst == AVAuthorizationStatusAuthorized) afterMic(YES);
    else if (mst == AVAuthorizationStatusNotDetermined) {
        [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:afterMic];
    } else afterMic(NO);
}

static NSString *jv_find_exec(NSArray<NSString *> *paths) {
    NSFileManager *fm = [NSFileManager defaultManager];
    for (NSString *p in paths) {
        if ([fm isExecutableFileAtPath:p]) return p;
    }
    return nil;
}

static NSString *jv_qwen_helper_path(void) {
    NSString *env = NSProcessInfo.processInfo.environment[@"JUXU_QWEN_HELPER"];
    if (env.length > 0 && [[NSFileManager defaultManager] isExecutableFileAtPath:env]) return env;
    NSString *dir = NSBundle.mainBundle.executableURL.URLByDeletingLastPathComponent.path;
    if (dir.length > 0) {
        NSString *p = [dir stringByAppendingPathComponent:@"juxu-qwen-asr"];
        if ([[NSFileManager defaultManager] isExecutableFileAtPath:p]) return p;
    }
    return nil;
}

static NSString *jv_models_dir(void) {
    return [NSHomeDirectory() stringByAppendingPathComponent:@"Library/Application Support/Juxu/VoiceModels"];
}

void *juxu_voice_probe_asr_status(void *nsPresetRaw, int64_t *outDepOk, int64_t *outModOk, int64_t *outBytes) {
    NSString *raw = jv_ns(nsPresetRaw);
    NSFileManager *fm = [NSFileManager defaultManager];
    if ([raw isEqualToString:@"apple_speech"] || [raw isEqualToString:@"custom_http"]) {
        *outDepOk = 1; *outModOk = 1; *outBytes = 0;
        return jv_ret_ns(@"|");
    }
    if ([raw isEqualToString:@"qwen3_asr"]) {
        NSString *dep = jv_qwen_helper_path();
        NSString *dir = [jv_models_dir() stringByAppendingPathComponent:@"Qwen3-ASR-0.6B-5eb1441"];
        NSString *marker = [dir stringByAppendingPathComponent:@".verified-5eb144179a02acc5e5ba31e748d22b0cf3e303b0"];
        BOOL modOk = [fm fileExistsAtPath:marker] && [fm fileExistsAtPath:[dir stringByAppendingPathComponent:@"model.safetensors"]];
        *outDepOk = dep != nil ? 1 : 0;
        *outModOk = modOk ? 1 : 0;
        *outBytes = modOk ? 1880559070LL : 0LL;
        return jv_ret_ns([NSString stringWithFormat:@"%@|%@", dep ?: @"", dir]);
    }
    NSString *file = [raw isEqualToString:@"whisper_base"] ? @"ggml-base.bin" : @"ggml-large-v3-turbo-q5_0.bin";
    NSString *dep = jv_find_exec(@[@"/opt/homebrew/bin/whisper-cli", @"/usr/local/bin/whisper-cli",
                                   @"/opt/homebrew/bin/whisper-cpp", @"/usr/local/bin/whisper-cpp"]);
    NSString *modPath = [jv_models_dir() stringByAppendingPathComponent:file];
    NSDictionary *attrs = [fm attributesOfItemAtPath:modPath error:nil];
    int64_t sz = [attrs[NSFileSize] longLongValue];
    *outDepOk = dep != nil ? 1 : 0;
    *outModOk = sz > 1000000LL ? 1 : 0;
    *outBytes = sz;
    return jv_ret_ns([NSString stringWithFormat:@"%@|%@", dep ?: @"", modPath]);
}

void *juxu_voice_probe_llm_status(void *nsPresetRaw, int64_t *outDepOk, int64_t *outModOk, int64_t *outBytes) {
    NSString *raw = jv_ns(nsPresetRaw);
    if (![raw isEqualToString:@"qwen_1_5b"]) {
        *outDepOk = 1; *outModOk = 1; *outBytes = 0;
        return jv_ret_ns(@"|");
    }
    NSString *dep = jv_find_exec(@[@"/opt/homebrew/bin/llama-cli", @"/usr/local/bin/llama-cli",
                                   @"/opt/homebrew/bin/llama-completion", @"/usr/local/bin/llama-completion"]);
    NSString *modPath = [jv_models_dir() stringByAppendingPathComponent:@"qwen2.5-1.5b-instruct-q4_k_m.gguf"];
    int64_t sz = [[[NSFileManager defaultManager] attributesOfItemAtPath:modPath error:nil][NSFileSize] longLongValue];
    *outDepOk = dep != nil ? 1 : 0;
    *outModOk = sz > 1000000LL ? 1 : 0;
    *outBytes = sz;
    return jv_ret_ns([NSString stringWithFormat:@"%@|%@", dep ?: @"", modPath]);
}

void juxu_voice_delete_model_file(void *nsFilename) {
    NSString *name = jv_ns(nsFilename);
    if (name.length == 0) return;
    if ([name isEqualToString:@"Qwen3-ASR-0.6B-4bit"]) name = @"Qwen3-ASR-0.6B-5eb1441";
    NSString *p = [jv_models_dir() stringByAppendingPathComponent:name];
    [[NSFileManager defaultManager] removeItemAtPath:p error:nil];
    [[NSFileManager defaultManager] removeItemAtPath:[p stringByAppendingPathExtension:@"part"] error:nil];
}

int64_t juxu_voice_check_apple_speech(void *nsLocale, bool requireOnDevice) {
    NSString *locId = jv_ns(nsLocale);
    if (locId.length == 0) locId = @"zh-CN";
    SFSpeechRecognizer *rec = [[SFSpeechRecognizer alloc] initWithLocale:[NSLocale localeWithLocaleIdentifier:locId]];
    if (!rec) return -1;
    if (requireOnDevice && !rec.supportsOnDeviceRecognition) return -2;
    return 0;
}

/* ---- 5. Active Audio Recorder + Apple Speech + Multipart/Chat Pipeline ---- */

@interface JuxuVoiceRecorderBox : NSObject {
@public
    NSLock *_lock;
    AVAudioEngine *_engine;
    AVAudioFormat *_targetFormat;
    AVAudioConverter *_converter;
    SFSpeechRecognizer *_speechRec;
    SFSpeechAudioBufferRecognitionRequest *_speechReq;
    SFSpeechRecognitionTask *_speechTask;
    NSMutableData *_pcm16;
    NSString *_latestApple;
    BOOL _appleDone;
    double _muteStart;
    double _muteEnd;
    float _noiseFloor;
    float _smoothedLevel;
    int _hist[91];
}
@end

@implementation JuxuVoiceRecorderBox
- (instancetype)init {
    if ((self = [super init])) {
        _lock = [[NSLock alloc] init];
        _pcm16 = [NSMutableData dataWithCapacity:16000 * 30 * 2];
        _latestApple = @"";
        _noiseFloor = 0.004f;
    }
    return self;
}
@end

static JuxuVoiceRecorderBox *g_rec = nil;

void juxu_voice_recorder_cancel(void) {
    JuxuVoiceRecorderBox *r = g_rec;
    g_rec = nil;
    if (!r) return;
    [r->_lock lock];
    AVAudioEngine *eng = r->_engine; r->_engine = nil;
    SFSpeechAudioBufferRecognitionRequest *req = r->_speechReq; r->_speechReq = nil;
    SFSpeechRecognitionTask *task = r->_speechTask; r->_speechTask = nil;
    r->_speechRec = nil;
    [r->_lock unlock];
    if (eng) { [eng.inputNode removeTapOnBus:0]; [eng stop]; }
    [req endAudio];
    [task cancel];
}

void juxu_voice_recorder_mute(double seconds) {
    JuxuVoiceRecorderBox *r = g_rec;
    if (!r) return;
    double now = [AVAudioTime secondsForHostTime:mach_absolute_time()];
    [r->_lock lock];
    r->_muteStart = now;
    r->_muteEnd = now + seconds;
    [r->_lock unlock];
}

void juxu_voice_recorder_start(
    bool useAppleSpeech,
    void *nsLocale,
    bool requireOnDevice,
    void *nsContextJoined,
    void *onStartFn, int64_t startHandle,
    void *onLevelFn, int64_t levelHandle
) {
    juxu_voice_recorder_cancel();
    JuxuVoiceRecorderBox *box = [[JuxuVoiceRecorderBox alloc] init];
    g_rec = box;
    NSString *locStr = jv_ns(nsLocale);
    NSString *ctxJoined = jv_ns(nsContextJoined);
    juxu_voice_cb_ok_str_t startCb = (juxu_voice_cb_ok_str_t)onStartFn;
    juxu_voice_cb_level_t levelCb = (juxu_voice_cb_level_t)onLevelFn;

    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        AVAudioEngine *eng = [[AVAudioEngine alloc] init];
        AVAudioInputNode *inNode = eng.inputNode;
        AVAudioFormat *inFmt = [inNode outputFormatForBus:0];
        if (inFmt.sampleRate <= 0 || inFmt.channelCount == 0) {
            dispatch_async(dispatch_get_main_queue(), ^{
                if (startCb) startCb(startHandle, false, jv_ret_ns(@"请在「系统设置 → 隐私与安全性 → 麦克风」中允许句序访问麦克风"));
                gc_handle_free(startHandle);
            });
            return;
        }
        AVAudioFormat *outFmt = [[AVAudioFormat alloc] initWithCommonFormat:AVAudioPCMFormatInt16 sampleRate:16000 channels:1 interleaved:YES];
        AVAudioConverter *conv = [[AVAudioConverter alloc] initFromFormat:inFmt toFormat:outFmt];
        box->_targetFormat = outFmt;
        box->_converter = conv;

        if (useAppleSpeech) {
            NSString *lid = locStr.length > 0 ? locStr : @"zh-CN";
            SFSpeechRecognizer *rec = [[SFSpeechRecognizer alloc] initWithLocale:[NSLocale localeWithLocaleIdentifier:lid]];
            if (!rec || !rec.isAvailable) {
                dispatch_async(dispatch_get_main_queue(), ^{
                    if (startCb) startCb(startHandle, false, jv_ret_ns(@"Apple 本地语音识别当前不可用"));
                    gc_handle_free(startHandle);
                });
                return;
            }
            SFSpeechAudioBufferRecognitionRequest *req = [[SFSpeechAudioBufferRecognitionRequest alloc] init];
            req.shouldReportPartialResults = YES;
            req.addsPunctuation = YES;
            if (ctxJoined.length > 0) req.contextualStrings = [ctxJoined componentsSeparatedByString:@"\n"];
            if (requireOnDevice) {
                if (!rec.supportsOnDeviceRecognition) {
                    dispatch_async(dispatch_get_main_queue(), ^{
                        if (startCb) startCb(startHandle, false, jv_ret_ns(@"未检测到离线语音模型，请在系统设置 → 键盘 → 听写中下载，或取消勾选「仅离线」"));
                        gc_handle_free(startHandle);
                    });
                    return;
                }
                req.requiresOnDeviceRecognition = YES;
            }
            box->_speechRec = rec;
            box->_speechReq = req;
            box->_speechTask = [rec recognitionTaskWithRequest:req resultHandler:^(SFSpeechRecognitionResult *result, NSError *err) {
                if (result) {
                    NSString *txt = result.bestTranscription.formattedString ?: @"";
                    [box->_lock lock];
                    box->_latestApple = txt;
                    if (result.isFinal) box->_appleDone = YES;
                    [box->_lock unlock];
                    dispatch_async(dispatch_get_main_queue(), ^{
                        if (g_rec == box && levelCb) levelCb(levelHandle, -1.0, jv_ret_ns(txt));
                    });
                } else if (err) {
                    [box->_lock lock];
                    box->_appleDone = YES;
                    [box->_lock unlock];
                }
            }];
        }

        [inNode installTapOnBus:0 bufferSize:1024 format:inFmt block:^(AVAudioPCMBuffer *buffer, AVAudioTime *when) {
            [box->_lock lock];
            SFSpeechAudioBufferRecognitionRequest *req = box->_speechReq;
            AVAudioConverter *cv = box->_converter;
            AVAudioFormat *tf = box->_targetFormat;
            [box->_lock unlock];
            if (req) [req appendAudioPCMBuffer:buffer];
            if (buffer.floatChannelData && buffer.frameLength > 0) {
                const float *ch = buffer.floatChannelData[0];
                int cnt = (int)buffer.frameLength;
                float sum = 0.0f;
                for (int i = 0; i < cnt; i++) sum += ch[i] * ch[i];
                float rms = sqrtf(sum / (float)cnt);
                int bucket = (int)llround(20.0 * log10(fmax((double)rms, 1e-6))) + 90;
                if (bucket < 0) bucket = 0; if (bucket > 90) bucket = 90;
                [box->_lock lock];
                box->_hist[bucket]++;
                if (rms < box->_noiseFloor) box->_noiseFloor = box->_noiseFloor * 0.9f + rms * 0.1f;
                else box->_noiseFloor = box->_noiseFloor * 0.995f + rms * 0.005f;
                float snrDb = 20.0f * log10f(fmaxf(rms, 1e-6f) / fmaxf(box->_noiseFloor, 0.0015f));
                float norm = fmaxf(0.0f, fminf(1.0f, (snrDb - 3.0f) / 26.0f));
                box->_smoothedLevel = norm > box->_smoothedLevel ? norm * 0.65f + box->_smoothedLevel * 0.35f
                                                                 : norm * 0.25f + box->_smoothedLevel * 0.75f;
                float lvl = box->_smoothedLevel;
                [box->_lock unlock];
                dispatch_async(dispatch_get_main_queue(), ^{
                    if (g_rec == box && levelCb) levelCb(levelHandle, (double)lvl, NULL);
                });
            }
            if (!cv || !tf) return;
            AVAudioFrameCount cap = (AVAudioFrameCount)ceil((double)buffer.frameLength * (tf.sampleRate / buffer.format.sampleRate)) + 16;
            AVAudioPCMBuffer *outBuf = [[AVAudioPCMBuffer alloc] initWithPCMFormat:tf frameCapacity:cap];
            __block BOOL fed = NO;
            NSError *convErr = nil;
            [cv convertToBuffer:outBuf error:&convErr withInputFromBlock:^AVAudioBuffer *(AVAudioPacketCount n, AVAudioConverterInputStatus *st) {
                if (fed) { *st = AVAudioConverterInputStatus_NoDataNow; return nil; }
                fed = YES; *st = AVAudioConverterInputStatus_HaveData; return buffer;
            }];
            if (!convErr && outBuf.frameLength > 0 && outBuf.int16ChannelData) {
                int16_t *p = outBuf.int16ChannelData[0];
                size_t n = (size_t)outBuf.frameLength;
                [box->_lock lock];
                size_t curSamples = box->_pcm16.length / 2;
                if (curSamples < 16000 * 300) {
                    size_t add = (curSamples + n <= 16000 * 300) ? n : (16000 * 300 - curSamples);
                    if (when.isHostTimeValid && box->_muteEnd > box->_muteStart) {
                        double bs = [AVAudioTime secondsForHostTime:when.hostTime];
                        int m0 = (int)llround((box->_muteStart - bs) * 16000.0);
                        int m1 = (int)llround((box->_muteEnd - bs) * 16000.0);
                        if (m0 < 0) m0 = 0; if (m1 > (int)add) m1 = (int)add;
                        for (int k = m0; k < m1; k++) p[k] = 0;
                    }
                    [box->_pcm16 appendBytes:p length:add * 2];
                }
                [box->_lock unlock];
            }
        }];

        box->_engine = eng;
        [eng prepare];
        NSError *startErr = nil;
        BOOL ok = [eng startAndReturnError:&startErr];
        dispatch_async(dispatch_get_main_queue(), ^{
            if (!ok) {
                juxu_voice_recorder_cancel();
                if (startCb) startCb(startHandle, false, jv_ret_ns(startErr.localizedDescription ?: @"音频引擎启动失败"));
            } else {
                if (startCb) startCb(startHandle, true, jv_ret_ns(@""));
            }
            gc_handle_free(startHandle);
        });
    });
}

/* Stops audio engine & runs non-blocking transcription (`cloud` HTTP or `local` engine/CLI/AppleSpeech) */
static NSData *jv_multipart_wav(NSString *boundary, NSData *wav, NSString *model, NSString *lang, NSString *prompt) {
    NSMutableData *body = [NSMutableData dataWithCapacity:wav.length + 512];
    void (^field)(NSString *, NSString *) = ^(NSString *k, NSString *v) {
        if (!v || v.length == 0) return;
        [body appendData:[[NSString stringWithFormat:@"--%@\r\nContent-Disposition: form-data; name=\"%@\"\r\n\r\n%@\r\n", boundary, k, v] dataUsingEncoding:NSUTF8StringEncoding]];
    };
    field(@"model", model);
    if (lang.length > 0 && ![lang isEqualToString:@"auto"]) field(@"language", lang);
    if (prompt.length > 0) field(@"prompt", prompt);
    field(@"response_format", @"json");
    [body appendData:[[NSString stringWithFormat:@"--%@\r\nContent-Disposition: form-data; name=\"file\"; filename=\"speech.wav\"\r\nContent-Type: audio/wav\r\n\r\n", boundary] dataUsingEncoding:NSUTF8StringEncoding]];
    [body appendData:wav];
    [body appendData:[[NSString stringWithFormat:@"\r\n--%@--\r\n", boundary] dataUsingEncoding:NSUTF8StringEncoding]];
    return body;
}

static NSString *jv_sync_subprocess(NSString *bin, NSArray<NSString *> *args, int *outExit, NSString **outErr) {
    NSTask *task = [[NSTask alloc] init];
    task.executableURL = [NSURL fileURLWithPath:bin];
    task.arguments = args;
    task.standardInput = [NSFileHandle fileHandleWithNullDevice];
    NSPipe *outP = [NSPipe pipe], *errP = [NSPipe pipe];
    task.standardOutput = outP; task.standardError = errP;
    NSError *err = nil;
    if (![task launchAndReturnError:&err]) {
        if (outExit) *outExit = -1;
        if (outErr) *outErr = err.localizedDescription ?: @"启动失败";
        return @"";
    }
    __block NSData *outD = nil, *errD = nil;
    dispatch_group_t grp = dispatch_group_create();
    dispatch_group_async(grp, dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
        outD = [outP.fileHandleForReading readDataToEndOfFile];
    });
    dispatch_group_async(grp, dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
        errD = [errP.fileHandleForReading readDataToEndOfFile];
    });
    [task waitUntilExit];
    dispatch_group_wait(grp, DISPATCH_TIME_FOREVER);
    if (outExit) *outExit = task.terminationStatus;
    if (outErr) *outErr = [[NSString alloc] initWithData:errD encoding:NSUTF8StringEncoding] ?: @"";
    return [[NSString alloc] initWithData:outD encoding:NSUTF8StringEncoding] ?: @"";
}

static NSString *jv_http_transcribe_sync(NSString *baseURL, NSString *apiKey, NSString *model, NSString *lang, NSString *prompt, NSData *wav, NSString **outErr) {
    NSString *base = [[baseURL stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet]
                      stringByTrimmingCharactersInSet:[NSCharacterSet characterSetWithCharactersInString:@"/"]];
    NSURL *url = [NSURL URLWithString:[base stringByAppendingString:@"/audio/transcriptions"]];
    if (!url) { if (outErr) *outErr = @"接口地址无效"; return nil; }
    NSString *boundary = [NSString stringWithFormat:@"JuxuVoice-%@", NSUUID.UUID.UUIDString];
    NSMutableURLRequest *req = [NSMutableURLRequest requestWithURL:url cachePolicy:NSURLRequestReloadIgnoringLocalCacheData timeoutInterval:30];
    req.HTTPMethod = @"POST";
    [req setValue:[NSString stringWithFormat:@"multipart/form-data; boundary=%@", boundary] forHTTPHeaderField:@"Content-Type"];
    if (apiKey.length > 0) [req setValue:[NSString stringWithFormat:@"Bearer %@", apiKey] forHTTPHeaderField:@"Authorization"];
    req.HTTPBody = jv_multipart_wav(boundary, wav, model, lang, prompt);

    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    __block NSData *respData = nil; __block NSURLResponse *resp = nil; __block NSError *netErr = nil;
    [[[NSURLSession sharedSession] dataTaskWithRequest:req completionHandler:^(NSData *d, NSURLResponse *r, NSError *e) {
        respData = d; resp = r; netErr = e;
        dispatch_semaphore_signal(sem);
    }] resume];
    dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
    if (netErr) { if (outErr) *outErr = netErr.localizedDescription; return nil; }
    NSInteger code = [(NSHTTPURLResponse *)resp statusCode];
    if (code < 200 || code >= 300) {
        NSString *msg = [[NSString alloc] initWithData:respData encoding:NSUTF8StringEncoding] ?: @"";
        if (outErr) *outErr = [NSString stringWithFormat:@"HTTP %ld：%@", (long)code, [msg substringToIndex:MIN(msg.length, 140)]];
        return nil;
    }
    NSDictionary *json = [NSJSONSerialization JSONObjectWithData:respData options:0 error:nil];
    if ([json isKindOfClass:NSDictionary.class] && [json[@"text"] isKindOfClass:NSString.class]) {
        return [json[@"text"] stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    }
    return [[[NSString alloc] initWithData:respData encoding:NSUTF8StringEncoding] stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
}

void juxu_voice_stop_and_transcribe(
    bool isShortPress,
    bool useAppleSpeech,
    bool usesLocalEngine,
    void *nsLocalPreset,
    void *nsBaseURL,
    void *nsApiKey,
    void *nsModel,
    void *nsLang,
    void *nsPrompt,
    void *fnPtr,
    int64_t cbHandle,
    double *outVoicedSec
) {
    JuxuVoiceRecorderBox *box = g_rec;
    g_rec = nil;
    juxu_voice_cb_ok_str_t cb = (juxu_voice_cb_ok_str_t)fnPtr;
    if (!box) {
        if (cb) cb(cbHandle, false, jv_ret_ns(@"已取消语音输入"));
        gc_handle_free(cbHandle);
        return;
    }
    NSString *preset = jv_ns(nsLocalPreset);
    NSString *baseURL = jv_ns(nsBaseURL);
    NSString *apiKey = jv_ns(nsApiKey);
    NSString *model = jv_ns(nsModel);
    NSString *lang = jv_ns(nsLang);
    NSString *prompt = jv_ns(nsPrompt);

    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        [NSThread sleepForTimeInterval:0.18]; /* releaseTailDelay */
        [box->_lock lock];
        AVAudioEngine *eng = box->_engine; box->_engine = nil;
        SFSpeechAudioBufferRecognitionRequest *req = box->_speechReq; box->_speechReq = nil;
        NSData *pcmData = [box->_pcm16 copy];
        [box->_pcm16 setLength:0];
        [box->_lock unlock];
        if (eng) { [eng.inputNode removeTapOnBus:0]; [eng stop]; }
        [req endAudio];

        const int16_t *samples = (const int16_t *)pcmData.bytes;
        size_t nSamples = pcmData.length / 2;
        double voiced = jv_voiced_seconds(samples, nSamples, true, NULL);
        if (outVoicedSec) *outVoicedSec = voiced;

        if (isShortPress && voiced <= 0.0) {
            dispatch_async(dispatch_get_main_queue(), ^{
                if (cb) cb(cbHandle, false, jv_ret_ns(@"SILENT_TAP"));
                gc_handle_free(cbHandle);
            });
            return;
        }
        if (!isShortPress && nSamples < 3200 && voiced <= 0.0) {
            dispatch_async(dispatch_get_main_queue(), ^{
                if (cb) cb(cbHandle, false, jv_ret_ns(@"录音过短，已忽略"));
                gc_handle_free(cbHandle);
            });
            return;
        }

        if (useAppleSpeech) {
            for (int w = 0; w < 35; w++) {
                [box->_lock lock]; BOOL done = box->_appleDone; [box->_lock unlock];
                if (done) break;
                [NSThread sleepForTimeInterval:0.1];
            }
            [box->_lock lock];
            NSString *res = box->_latestApple ?: @"";
            SFSpeechRecognitionTask *t = box->_speechTask; box->_speechTask = nil; box->_speechRec = nil;
            [box->_lock unlock];
            [t cancel];
            dispatch_async(dispatch_get_main_queue(), ^{
                if (cb) cb(cbHandle, true, jv_ret_ns(res));
                gc_handle_free(cbHandle);
            });
            return;
        }

        if (usesLocalEngine && voiced <= 0.0) {
            dispatch_async(dispatch_get_main_queue(), ^{
                if (cb) cb(cbHandle, false, jv_ret_ns(@"没听清"));
                gc_handle_free(cbHandle);
            });
            return;
        }

        NSData *wav = jv_condition_and_encode_wav(samples, nSamples);
        NSString *errStr = nil;
        NSString *transcript = nil;

        if (usesLocalEngine) {
            if ([preset isEqualToString:@"qwen3_asr"]) {
                NSString *helper = jv_qwen_helper_path();
                NSString *modDir = [jv_models_dir() stringByAppendingPathComponent:@"Qwen3-ASR-0.6B-5eb1441"];
                if (helper && [[NSFileManager defaultManager] fileExistsAtPath:modDir]) {
                    NSTask *qt = [[NSTask alloc] init];
                    qt.executableURL = [NSURL fileURLWithPath:helper];
                    qt.arguments = @[modDir];
                    NSPipe *inP = [NSPipe pipe], *outP = [NSPipe pipe];
                    qt.standardInput = inP; qt.standardOutput = outP;
                    qt.standardError = [NSFileHandle fileHandleWithNullDevice];
                    if ([qt launchAndReturnError:nil]) {
                        NSMutableDictionary *hdr = [@{@"id": @1, @"kind": @"final", @"samples": @(nSamples)} mutableCopy];
                        if ([lang hasPrefix:@"zh"]) hdr[@"language"] = @"Chinese";
                        else if ([lang hasPrefix:@"en"]) hdr[@"language"] = @"English";
                        if (prompt.length > 0) hdr[@"context"] = prompt;
                        NSMutableData *pl = [[NSJSONSerialization dataWithJSONObject:hdr options:0 error:nil] mutableCopy];
                        uint8_t nl = 0x0A; [pl appendBytes:&nl length:1];
                        [pl appendData:[wav subdataWithRange:NSMakeRange(44, wav.length - 44)]];
                        [inP.fileHandleForWriting writeData:pl error:nil];
                        [inP.fileHandleForWriting closeFile];
                        NSData *outD = [outP.fileHandleForReading readDataToEndOfFile];
                        [qt waitUntilExit];
                        NSArray *lines = [[[NSString alloc] initWithData:outD encoding:NSUTF8StringEncoding] componentsSeparatedByString:@"\n"];
                        for (NSString *ln in lines) {
                            if (ln.length == 0) continue;
                            NSDictionary *obj = [NSJSONSerialization JSONObjectWithData:[ln dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil];
                            if ([obj[@"text"] isKindOfClass:NSString.class]) transcript = obj[@"text"];
                        }
                    }
                }
            }
            if (!transcript) {
                NSString *wBin = jv_find_exec(@[@"/opt/homebrew/bin/whisper-cli", @"/usr/local/bin/whisper-cli",
                                                @"/opt/homebrew/bin/whisper-cpp", @"/usr/local/bin/whisper-cpp"]);
                NSString *mFile = [preset isEqualToString:@"whisper_base"] ? @"ggml-base.bin" : @"ggml-large-v3-turbo-q5_0.bin";
                NSString *mPath = [jv_models_dir() stringByAppendingPathComponent:mFile];
                if (![[NSFileManager defaultManager] fileExistsAtPath:mPath]) {
                    mPath = [jv_models_dir() stringByAppendingPathComponent:@"ggml-base.bin"];
                }
                if (!wBin || ![[NSFileManager defaultManager] fileExistsAtPath:mPath]) {
                    errStr = @"所选语音模型尚未安装，请在「句序偏好设置 → 语音输入」中下载";
                } else {
                    NSString *tmpWav = [NSTemporaryDirectory() stringByAppendingPathComponent:[NSString stringWithFormat:@"juxu-%@.wav", NSUUID.UUID.UUIDString]];
                    [wav writeToFile:tmpWav atomically:YES];
                    NSMutableArray *args = [@[@"-f", tmpWav, @"--no-timestamps", @"-nt", @"-m", mPath] mutableCopy];
                    if (lang.length > 0 && ![lang isEqualToString:@"auto"]) { [args addObject:@"-l"]; [args addObject:lang]; }
                    if (prompt.length > 0) { [args addObject:@"--prompt"]; [args addObject:prompt]; }
                    int ec = 0; NSString *se = nil;
                    NSString *outS = jv_sync_subprocess(wBin, args, &ec, &se);
                    [[NSFileManager defaultManager] removeItemAtPath:tmpWav error:nil];
                    if (ec == 0) transcript = [outS stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
                    else errStr = [NSString stringWithFormat:@"本地语音执行失败：%@", [se substringToIndex:MIN(se.length, 140)]];
                }
            }
        } else {
            transcript = jv_http_transcribe_sync(baseURL, apiKey, model, lang, prompt, wav, &errStr);
        }

        dispatch_async(dispatch_get_main_queue(), ^{
            if (transcript != nil) {
                if (cb) cb(cbHandle, true, jv_ret_ns([NSString stringWithFormat:@"%.3f\t%@", voiced, transcript]));
            } else {
                if (cb) cb(cbHandle, false, jv_ret_ns(errStr ?: @"未识别到有效语音内容"));
            }
            gc_handle_free(cbHandle);
        });
    });
}

/* ---- 6. Cloud / Local HTTP & Llama CLI Polish (`VoicePipelineClient.polish*`) ---- */

void juxu_voice_polish_async(
    bool useLocalLlamaCLI,
    void *nsBaseOrBin,
    void *nsKeyOrModelPath,
    void *nsModelName,
    void *nsSysPrompt,
    void *nsUserPrompt,
    void *fnPtr,
    int64_t cbHandle
) {
    NSString *baseOrBin = jv_ns(nsBaseOrBin);
    NSString *keyOrPath = jv_ns(nsKeyOrModelPath);
    NSString *model = jv_ns(nsModelName);
    NSString *sysP = jv_ns(nsSysPrompt);
    NSString *usrP = jv_ns(nsUserPrompt);
    juxu_voice_cb_ok_str_t cb = (juxu_voice_cb_ok_str_t)fnPtr;

    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        if (useLocalLlamaCLI) {
            NSString *chatML = [NSString stringWithFormat:@"<|im_start|>system\n%@<|im_end|>\n<|im_start|>user\n%@<|im_end|>\n<|im_start|>assistant\n", sysP, usrP];
            int ec = 0; NSString *se = nil;
            NSString *out = jv_sync_subprocess(baseOrBin, @[@"-m", keyOrPath, @"-ngl", @"99", @"-c", @"2048", @"-n", @"512",
                                                            @"--temp", @"0.1", @"--no-display-prompt", @"-no-cnv", @"-p", chatML], &ec, &se);
            for (NSString *mk in @[@"<|im_end|>", @"[end of text]", @"<|endoftext|>"]) {
                out = [out stringByReplacingOccurrencesOfString:mk withString:@""];
            }
            NSString *clean = [out stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
            dispatch_async(dispatch_get_main_queue(), ^{
                if (ec == 0) { if (cb) cb(cbHandle, true, jv_ret_ns(clean)); }
                else { if (cb) cb(cbHandle, false, jv_ret_ns(se ?: @"本地 LLM 执行失败")); }
                gc_handle_free(cbHandle);
            });
            return;
        }
        NSString *base = [[baseOrBin stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet]
                          stringByTrimmingCharactersInSet:[NSCharacterSet characterSetWithCharactersInString:@"/"]];
        NSURL *url = [NSURL URLWithString:[base stringByAppendingString:@"/chat/completions"]];
        if (!url) {
            dispatch_async(dispatch_get_main_queue(), ^{
                if (cb) cb(cbHandle, false, jv_ret_ns(@"接口地址无效"));
                gc_handle_free(cbHandle);
            });
            return;
        }
        NSDictionary *payload = @{
            @"model": model,
            @"temperature": @0.1,
            @"messages": @[@{@"role": @"system", @"content": sysP}, @{@"role": @"user", @"content": usrP}]
        };
        NSMutableURLRequest *req = [NSMutableURLRequest requestWithURL:url cachePolicy:NSURLRequestReloadIgnoringLocalCacheData timeoutInterval:30];
        req.HTTPMethod = @"POST";
        [req setValue:@"application/json" forHTTPHeaderField:@"Content-Type"];
        if (keyOrPath.length > 0) [req setValue:[NSString stringWithFormat:@"Bearer %@", keyOrPath] forHTTPHeaderField:@"Authorization"];
        req.HTTPBody = [NSJSONSerialization dataWithJSONObject:payload options:0 error:nil];
        [[[NSURLSession sharedSession] dataTaskWithRequest:req completionHandler:^(NSData *d, NSURLResponse *r, NSError *e) {
            NSInteger code = [(NSHTTPURLResponse *)r statusCode];
            NSString *content = nil; NSString *errMsg = e.localizedDescription;
            if (!e && code >= 200 && code < 300 && d) {
                NSDictionary *j = [NSJSONSerialization JSONObjectWithData:d options:0 error:nil];
                NSArray *ch = j[@"choices"];
                if ([ch isKindOfClass:NSArray.class] && ch.count > 0) {
                    content = [ch[0][@"message"][@"content"] description];
                }
            } else if (!errMsg) {
                NSString *raw = [[NSString alloc] initWithData:d encoding:NSUTF8StringEncoding] ?: @"";
                errMsg = [NSString stringWithFormat:@"HTTP %ld：%@", (long)code, [raw substringToIndex:MIN(raw.length, 140)]];
            }
            dispatch_async(dispatch_get_main_queue(), ^{
                if (content) { if (cb) cb(cbHandle, true, jv_ret_ns([content stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet])); }
                else { if (cb) cb(cbHandle, false, jv_ret_ns(errMsg ?: @"整理接口返回格式无法解析")); }
                gc_handle_free(cbHandle);
            });
        }] resume];
    });
}

/* ---- 7. One-Click Model & Brew Installer (`VoiceLocalInstaller`) ---- */

static volatile int32_t g_install_cancel = 0;
void juxu_voice_cancel_install(void) { g_install_cancel = 1; }

static BOOL jv_sha256_matches(NSString *path, NSString *expectedHex) {
    if (!expectedHex || expectedHex.length == 0) return YES;
    NSFileHandle *fh = [NSFileHandle fileHandleForReadingAtPath:path];
    if (!fh) return NO;
    CC_SHA256_CTX ctx; CC_SHA256_Init(&ctx);
    while (!g_install_cancel) {
        @autoreleasepool {
            NSData *chunk = [fh readDataOfLength:4 << 20];
            if (chunk.length == 0) break;
            CC_SHA256_Update(&ctx, chunk.bytes, (CC_LONG)chunk.length);
        }
    }
    [fh closeFile];
    if (g_install_cancel) return NO;
    unsigned char digest[CC_SHA256_DIGEST_LENGTH];
    CC_SHA256_Final(digest, &ctx);
    NSMutableString *hex = [NSMutableString stringWithCapacity:64];
    for (int i = 0; i < CC_SHA256_DIGEST_LENGTH; i++) [hex appendFormat:@"%02x", digest[i]];
    return [hex isEqualToString:[expectedHex lowercaseString]];
}

void juxu_voice_install_component(
    bool isLLM,
    void *nsPresetRaw,
    void *nsDisplayName,
    void *progFnPtr, int64_t progHandle
) {
    g_install_cancel = 0;
    NSString *raw = jv_ns(nsPresetRaw);
    NSString *disp = jv_ns(nsDisplayName);
    juxu_voice_cb_prog_t cb = (juxu_voice_cb_prog_t)progFnPtr;
    void (^report)(int32_t, double, NSString *, NSString *, BOOL) = ^(int32_t k, double p, NSString *nm, NSString *dt, BOOL terminal) {
        dispatch_async(dispatch_get_main_queue(), ^{
            if (cb) cb(progHandle, k, p, jv_ret_ns(nm), jv_ret_ns(dt));
            if (terminal) gc_handle_free(progHandle);
        });
    };

    dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
        NSFileManager *fm = [NSFileManager defaultManager];
        [fm createDirectoryAtPath:jv_models_dir() withIntermediateDirectories:YES attributes:nil error:nil];

        NSString *brewFormula = nil;
        NSString *file = nil;
        NSString *repo = nil;
        NSString *sha = nil;
        if (isLLM) {
            if (![raw isEqualToString:@"qwen_1_5b"]) { report(3, 1.0, disp, @"已就绪", YES); return; }
            if (!jv_find_exec(@[@"/opt/homebrew/bin/llama-cli", @"/usr/local/bin/llama-cli", @"/opt/homebrew/bin/llama-completion"]))
                brewFormula = @"llama.cpp";
            file = @"qwen2.5-1.5b-instruct-q4_k_m.gguf";
            repo = @"Qwen/Qwen2.5-1.5B-Instruct-GGUF/resolve/main";
            sha = @"6a1a2eb6d15622bf3c96857206351ba97e1af16c30d7a74ee38970e434e9407e";
        } else if ([raw isEqualToString:@"whisper_turbo"] || [raw isEqualToString:@"whisper_base"]) {
            if (!jv_find_exec(@[@"/opt/homebrew/bin/whisper-cli", @"/usr/local/bin/whisper-cli", @"/opt/homebrew/bin/whisper-cpp"]))
                brewFormula = @"whisper.cpp";
            BOOL base = [raw isEqualToString:@"whisper_base"];
            file = base ? @"ggml-base.bin" : @"ggml-large-v3-turbo-q5_0.bin";
            repo = @"ggerganov/whisper.cpp/resolve/main";
            sha = base ? @"60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe"
                       : @"394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2";
        } else {
            report(4, 0.0, disp, @"请安装内置 Qwen3-ASR 组件或选择 Whisper 模型", YES);
            return;
        }

        if (brewFormula) {
            NSString *brew = jv_find_exec(@[@"/opt/homebrew/bin/brew", @"/usr/local/bin/brew"]);
            if (!brew) { report(4, 0.0, disp, @"未检测到 Homebrew，请先安装 Homebrew（https://brew.sh）", YES); return; }
            report(1, 0.0, brewFormula, @"", NO);
            int ec = 0; NSString *se = nil;
            jv_sync_subprocess(brew, @[@"install", brewFormula], &ec, &se);
            if (g_install_cancel) { report(0, 0.0, @"", @"", YES); return; }
            if (ec != 0) { report(4, 0.0, disp, [NSString stringWithFormat:@"brew install %@ 失败：%@", brewFormula, se], YES); return; }
        }

        NSString *dest = [jv_models_dir() stringByAppendingPathComponent:file];
        if ([[fm attributesOfItemAtPath:dest error:nil][NSFileSize] longLongValue] > 1000000LL) {
            report(3, 1.0, disp, [NSString stringWithFormat:@"✓ %@ 与依赖已就绪", disp], YES);
            return;
        }
        NSArray *mirrors = @[@"https://huggingface.co", @"https://hf-mirror.com"];
        BOOL ok = NO;
        for (NSUInteger mi = 0; mi < mirrors.count && !ok && !g_install_cancel; mi++) {
            NSString *tag = mi == 0 ? @"HuggingFace" : @"镜像源";
            NSString *uStr = [NSString stringWithFormat:@"%@/%@/%@", mirrors[mi], repo, file];
            NSString *part = [dest stringByAppendingPathExtension:@"part"];
            [fm removeItemAtPath:part error:nil];
            report(2, 0.02, disp, [NSString stringWithFormat:@"连接 %@…", tag], NO);
            int ec = 0; NSString *se = nil;
            /* Limit rate to 2.5MB/s per MEMORY.md rule for >1GB home-network friendliness when LLM */
            NSMutableArray *cArgs = [@[@"-fL", @"-o", part, uStr] mutableCopy];
            if (isLLM) { [cArgs insertObject:@"--limit-rate" atIndex:0]; [cArgs insertObject:@"2500k" atIndex:1]; }
            jv_sync_subprocess(@"/usr/bin/curl", cArgs, &ec, &se);
            if (ec == 0 && [fm fileExistsAtPath:part]) {
                report(2, 1.0, disp, @"校验 SHA-256…", NO);
                if (jv_sha256_matches(part, sha)) {
                    [fm removeItemAtPath:dest error:nil];
                    [fm moveItemAtPath:part toPath:dest error:nil];
                    ok = YES;
                } else {
                    [fm removeItemAtPath:part error:nil];
                }
            }
        }
        if (g_install_cancel) report(0, 0.0, @"", @"", YES);
        else if (ok) report(3, 1.0, disp, [NSString stringWithFormat:@"✓ %@ 与依赖已就绪", disp], YES);
        else report(4, 0.0, disp, @"模型下载或校验失败，请重试", YES);
    });
}
