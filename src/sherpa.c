/**
 * @file sherpa.c
 * @brief Sherpa-ONNX voice engine worker for Typio
 *
 * Out-of-process worker started at runtime from an engine manifest.
 * Links against libtypio (ABI) and sherpa-onnx-c-api.
 */

#define _POSIX_C_SOURCE 200809L

#include <typio/abi/abi.h>
#include <typio/schema/config_schema.h>
#include <pthread.h>

#include <sherpa-onnx/c-api/c-api.h>
#include <curl/curl.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

typedef struct {
    const SherpaOnnxOfflineRecognizer *recognizer;
    TypioInstance *instance;
    char language[16];
    char model_hint[64];
} SherpaState;

/* ── Helpers ───────────────────────────────────────────────────────────── */

static bool file_exists(const char *path) {
    struct stat st;
    return stat(path, &st) == 0 && S_ISREG(st.st_mode);
}

static bool find_file_with_suffix(const char *dir, const char *suffix,
                                   char *buf, size_t buf_size) {
    DIR *d = opendir(dir);
    if (!d) {
        return false;
    }
    size_t suf_len = strlen(suffix);
    struct dirent *ent;
    while ((ent = readdir(d)) != NULL) {
        size_t name_len = strlen(ent->d_name);
        if (name_len >= suf_len &&
            strcmp(ent->d_name + name_len - suf_len, suffix) == 0) {
            snprintf(buf, buf_size, "%s/%s", dir, ent->d_name);
            closedir(d);
            return true;
        }
    }
    closedir(d);
    return false;
}

static bool find_model_file(const char *dir, const char *exact,
                            const char *suffix_fallback,
                            char *buf, size_t buf_size) {
    snprintf(buf, buf_size, "%s/%s", dir, exact);
    if (file_exists(buf)) {
        return true;
    }
    if (suffix_fallback) {
        return find_file_with_suffix(dir, suffix_fallback, buf, buf_size);
    }
    return false;
}

typedef enum {
    SHERPA_MODEL_UNKNOWN = 0,
    SHERPA_MODEL_SENSE_VOICE,
    SHERPA_MODEL_WHISPER,
    SHERPA_MODEL_TRANSDUCER,
    SHERPA_MODEL_PARAFORMER,
} SherpaModelType;

static SherpaModelType detect_model_type(const char *model_dir,
                                           char *tokens_buf,
                                           char *file1_buf,
                                           char *file2_buf,
                                           char *file3_buf,
                                           size_t buf_size) {
    if (!find_model_file(model_dir, "tokens.txt", "-tokens.txt",
                         tokens_buf, buf_size)) {
        return SHERPA_MODEL_UNKNOWN;
    }

    if (find_model_file(model_dir, "model.int8.onnx", NULL,
                        file1_buf, buf_size) ||
        find_model_file(model_dir, "model.onnx", NULL,
                        file1_buf, buf_size)) {
        if (strstr(model_dir, "sense-voice") ||
            strstr(model_dir, "sense_voice") ||
            strstr(model_dir, "sensevoice")) {
            return SHERPA_MODEL_SENSE_VOICE;
        }
        return SHERPA_MODEL_PARAFORMER;
    }

    if (find_model_file(model_dir, "joiner.onnx", "-joiner.onnx",
                        file3_buf, buf_size)) {
        if (find_model_file(model_dir, "encoder.onnx", "-encoder.onnx",
                            file1_buf, buf_size) &&
            find_model_file(model_dir, "decoder.onnx", "-decoder.onnx",
                            file2_buf, buf_size)) {
            return SHERPA_MODEL_TRANSDUCER;
        }
    }

    if (find_model_file(model_dir, "encoder.onnx", "-encoder.onnx",
                        file1_buf, buf_size) &&
        find_model_file(model_dir, "decoder.onnx", "-decoder.onnx",
                        file2_buf, buf_size)) {
        return SHERPA_MODEL_WHISPER;
    }

    return SHERPA_MODEL_UNKNOWN;
}

static const char *model_type_name(SherpaModelType type) {
    switch (type) {
    case SHERPA_MODEL_SENSE_VOICE: return "sense_voice";
    case SHERPA_MODEL_WHISPER:     return "whisper";
    case SHERPA_MODEL_TRANSDUCER:  return "transducer";
    case SHERPA_MODEL_PARAFORMER:  return "paraformer";
    default:                       return "unknown";
    }
}

static bool auto_detect_model_dir(const char *base_dir,
                                  char *model_dir, size_t dir_size,
                                  char *tokens_buf, char *f1, char *f2,
                                  char *f3, size_t buf_size,
                                  SherpaModelType *out_type) {
    DIR *d = opendir(base_dir);
    if (!d) {
        return false;
    }
    struct dirent *ent;
    while ((ent = readdir(d)) != NULL) {
        if (ent->d_name[0] == '.') {
            continue;
        }
        snprintf(model_dir, dir_size, "%s/%s", base_dir, ent->d_name);
        struct stat st;
        if (stat(model_dir, &st) != 0 || !S_ISDIR(st.st_mode)) {
            continue;
        }
        *out_type = detect_model_type(model_dir, tokens_buf, f1, f2, f3,
                                      buf_size);
        if (*out_type != SHERPA_MODEL_UNKNOWN) {
            closedir(d);
            return true;
        }
    }
    closedir(d);
    return false;
}

static void unload_model(SherpaState *state) {
    if (state->recognizer) {
        SherpaOnnxDestroyOfflineRecognizer(state->recognizer);
        state->recognizer = NULL;
    }
}

static bool load_model(SherpaState *state, TypioInstance *instance) {
    const char *data_dir = typio_instance_get_data_dir(instance);
    if (!data_dir) {
        typio_log_warning("sherpa-onnx: load_model: no data_dir");
        return false;
    }

    const char *language = "";
    const char *model_hint = "";
    TypioConfig *cfg = typio_instance_get_config(instance);
    if (cfg) {
        language = typio_config_get_string(cfg, "engines.sherpa-onnx.language", "");
        model_hint = typio_config_get_string(cfg, "engines.sherpa-onnx.model", "");
    }

    if (state->recognizer) {
        return true;
    }

    char base_dir[512];
    snprintf(base_dir, sizeof(base_dir), "%s/sherpa-onnx", data_dir);

    char model_dir[512];
    char tokens_path[512] = {};
    char file1[512] = {};
    char file2[512] = {};
    char file3[512] = {};
    SherpaModelType type = SHERPA_MODEL_UNKNOWN;

    if (model_hint && *model_hint) {
        int ret = snprintf(model_dir, sizeof(model_dir), "%s/%s", base_dir, model_hint);
        if (ret >= 0 && (size_t)ret < sizeof(model_dir)) {
            type = detect_model_type(model_dir, tokens_path, file1, file2, file3,
                                     sizeof(tokens_path));
        }
    }

    if (type == SHERPA_MODEL_UNKNOWN) {
        if (!auto_detect_model_dir(base_dir, model_dir, sizeof(model_dir),
                                   tokens_path, file1, file2, file3,
                                   sizeof(tokens_path), &type)) {
            typio_log_warning("sherpa-onnx: no recognizable model in %s",
                              base_dir);
            return false;
        }
    }

    typio_log_info("sherpa-onnx: detected model type '%s' in %s",
                   model_type_name(type), model_dir);

    SherpaOnnxOfflineRecognizerConfig config;
    memset(&config, 0, sizeof(config));
    config.feat_config.sample_rate = 16000;
    config.feat_config.feature_dim = 80;
    config.model_config.num_threads = 4;
    config.model_config.debug = 0;
    config.model_config.provider = "cpu";
    config.model_config.tokens = tokens_path;
    config.decoding_method = "greedy_search";

    switch (type) {
    case SHERPA_MODEL_SENSE_VOICE:
        config.model_config.sense_voice.model = file1;
        config.model_config.sense_voice.language = (language && *language) ? language : "auto";
        config.model_config.sense_voice.use_itn = 1;
        break;
    case SHERPA_MODEL_WHISPER:
        config.model_config.whisper.encoder = file1;
        config.model_config.whisper.decoder = file2;
        config.model_config.whisper.language = language;
        config.model_config.whisper.task = "transcribe";
        break;
    case SHERPA_MODEL_TRANSDUCER:
        config.model_config.transducer.encoder = file1;
        config.model_config.transducer.decoder = file2;
        config.model_config.transducer.joiner = file3;
        break;
    case SHERPA_MODEL_PARAFORMER:
        config.model_config.paraformer.model = file1;
        break;
    default:
        return false;
    }

    const SherpaOnnxOfflineRecognizer *recognizer =
        SherpaOnnxCreateOfflineRecognizer(&config);
    if (!recognizer) {
        typio_log_warning("sherpa-onnx: failed to create recognizer from %s",
                          model_dir);
        return false;
    }

    typio_log_info("sherpa-onnx: recognizer created (type=%s, dir=%s)",
                   model_type_name(type), model_dir);

    state->recognizer = recognizer;
    if (language) {
        strncpy(state->language, language, sizeof(state->language) - 1);
    }
    if (model_hint) {
        strncpy(state->model_hint, model_hint, sizeof(state->model_hint) - 1);
    }
    return true;
}

/* ── Model download ───────────────────────────────────────────────────── */

#define SHERPA_DEFAULT_MODEL_VERSION "2024-07-17"
#define SHERPA_MODEL_BASE_URL \
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models"

typedef struct {
    const char *name;
    const char *url_suffix;
    const char *files[4];
} SherpaInstallableModel;

static const SherpaInstallableModel sherpa_installable_models[] = {
    {
        .name = "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-" SHERPA_DEFAULT_MODEL_VERSION,
        .url_suffix = "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-"
                      SHERPA_DEFAULT_MODEL_VERSION ".tar.bz2",
        .files = {"model.int8.onnx", "tokens.txt", NULL},
    },
};

static size_t write_cb(void *ptr, size_t size, size_t nmemb, void *ud) {
    int fd = *(int *)ud;
    size_t total = size * nmemb;
    ssize_t written = write(fd, ptr, total);
    return (written < 0) ? 0 : (size_t)written;
}

static TypioResult download_to_tmp(const char *url, char *tmp_path, size_t path_size) {
    snprintf(tmp_path, path_size, "/tmp/typio-model-download-XXXXXX");
    int fd = mkstemp(tmp_path);
    if (fd < 0) {
        return TYPIO_ERROR;
    }

    CURL *curl = curl_easy_init();
    if (!curl) {
        close(fd);
        unlink(tmp_path);
        return TYPIO_ERROR;
    }

    curl_easy_setopt(curl, CURLOPT_URL, url);
    curl_easy_setopt(curl, CURLOPT_WRITEFUNCTION, write_cb);
    curl_easy_setopt(curl, CURLOPT_WRITEDATA, &fd);
    curl_easy_setopt(curl, CURLOPT_FOLLOWLOCATION, 1L);
    curl_easy_setopt(curl, CURLOPT_FAILONERROR, 1L);

    CURLcode res = curl_easy_perform(curl);
    curl_easy_cleanup(curl);
    close(fd);

    if (res != CURLE_OK) {
        unlink(tmp_path);
        return TYPIO_ERROR;
    }
    return TYPIO_OK;
}

static TypioResult extract_files(const char *tar_path,
                                  const char *dest_dir,
                                  const char *const *files) {
    mkdir(dest_dir, 0755);

    pid_t pid = fork();
    if (pid < 0) {
        return TYPIO_ERROR;
    }
    if (pid == 0) {
        execlp("tar", "tar", "xf", tar_path,
               "-C", "/tmp", "--no-anchored",
               (char *)NULL);
        _exit(127);
    }
    int status;
    waitpid(pid, &status, 0);
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        return TYPIO_ERROR;
    }

    char src_dir[512];
    const char *tmp = "/tmp";
    int found = 0;
    DIR *d = opendir(tmp);
    if (d) {
        struct dirent *ent;
        while ((ent = readdir(d))) {
            if (strstr(ent->d_name, "sherpa-onnx-sense-voice")) {
                snprintf(src_dir, sizeof(src_dir), "/tmp/%s", ent->d_name);
                found = 1;
                break;
            }
        }
        closedir(d);
    }
    if (!found) {
        return TYPIO_ERROR;
    }

    for (size_t i = 0; files[i]; i++) {
        char src[1024], dst[1024];
        snprintf(src, sizeof(src), "%s/%s", src_dir, files[i]);
        snprintf(dst, sizeof(dst), "%s/%s", dest_dir, files[i]);

        pid_t cp = fork();
        if (cp == 0) {
            execlp("cp", "cp", src, dst, (char *)NULL);
            _exit(127);
        }
        int cs;
        waitpid(cp, &cs, 0);
        if (!WIFEXITED(cs) || WEXITSTATUS(cs) != 0) {
            return TYPIO_ERROR;
        }
    }

    return TYPIO_OK;
}

static TypioResult install_default_model(TypioEngine *engine) {
    SherpaState *state = (SherpaState *)typio_engine_get_user_data(engine);
    if (!state || !state->instance) {
        return TYPIO_ERROR_NOT_INITIALIZED;
    }

    const SherpaInstallableModel *model = &sherpa_installable_models[0];

    const char *data_dir = typio_instance_get_data_dir(state->instance);
    if (!data_dir) {
        return TYPIO_ERROR_NOT_INITIALIZED;
    }

    char dest_dir[512];
    snprintf(dest_dir, sizeof(dest_dir), "%s/sherpa-onnx/%s",
             data_dir, model->name);

    struct stat st;
    if (stat(dest_dir, &st) == 0 && S_ISDIR(st.st_mode)) {
        typio_log_info("sherpa-onnx: model '%s' already installed at %s",
                       model->name, dest_dir);
        return TYPIO_OK;
    }

    char url[1024];
    snprintf(url, sizeof(url), "%s/%s", SHERPA_MODEL_BASE_URL, model->url_suffix);

    typio_log_info("sherpa-onnx: downloading %s", model->url_suffix);

    char tmp_path[512];
    TypioResult res = download_to_tmp(url, tmp_path, sizeof(tmp_path));
    if (res != TYPIO_OK) {
        typio_log_error("sherpa-onnx: download failed");
        return res;
    }

    res = extract_files(tmp_path, dest_dir, model->files);
    unlink(tmp_path);

    if (res != TYPIO_OK) {
        typio_log_error("sherpa-onnx: extraction failed");
        return res;
    }

    typio_log_info("sherpa-onnx: model installed to %s", dest_dir);
    return TYPIO_OK;
}

/* ── Config schema ──────────────────────────────────────────────────────── */

static const TypioConfigField sherpa_schema_fields[] = {
    {
        .key = "engines.sherpa-onnx.model",
        .type = TYPIO_FIELD_STRING,
        .def.s = "",
        .ui_label = "Model directory name",
        .ui_section = "voice",
    },
    {
        .key = "engines.sherpa-onnx.language",
        .type = TYPIO_FIELD_STRING,
        .def.s = "auto",
        .ui_label = "Language hint",
        .ui_section = "voice",
    },
};

TYPIO__EXTERN_C TYPIO_EXPORT const TypioConfigField *
typio_engine_get_config_schema(size_t *out_count) {
    if (out_count) {
        *out_count = sizeof(sherpa_schema_fields) / sizeof(sherpa_schema_fields[0]);
    }
    return sherpa_schema_fields;
}

/* ── Command surface (ADR-0008) ───────────────────────────────────────── */

typedef struct {
    TypioEngine *engine;
} SherpaSetupJob;

static void *sherpa_setup_thread(void *arg) {
    SherpaSetupJob *job = arg;
    TypioResult res = install_default_model(job->engine);
    if (res == TYPIO_OK) {
        SherpaState *state = (SherpaState *)typio_engine_get_user_data(job->engine);
        if (state && state->instance) {
            const SherpaInstallableModel *model = &sherpa_installable_models[0];
            typio_instance_set_engine_config_key(
                state->instance, "sherpa-onnx", "model", model->name);
        }
    }
    free(job);
    return NULL;
}

static const TypioEngineCommand sherpa_commands[] = {
    {.id = "setup", .label = "Download default model and configure"},
};

static const TypioEngineCommand *sherpa_list_commands(TypioEngine *engine,
                                                       size_t *out_count) {
    (void)engine;
    *out_count = sizeof(sherpa_commands) / sizeof(sherpa_commands[0]);
    return sherpa_commands;
}

static TypioResult sherpa_invoke_command(TypioEngine *engine, const char *id) {
    if (strcmp(id, "setup") == 0) {
        SherpaSetupJob *job = calloc(1, sizeof(SherpaSetupJob));
        if (!job) {
            return TYPIO_ERROR_OUT_OF_MEMORY;
        }
        job->engine = engine;
        pthread_t tid;
        if (pthread_create(&tid, NULL, sherpa_setup_thread, job) != 0) {
            free(job);
            return TYPIO_ERROR;
        }
        pthread_detach(tid);
        typio_log_info("sherpa-onnx: setup started in background");
        return TYPIO_OK;
    }
    return TYPIO_ERROR_NOT_FOUND;
}

static const TypioEngineSurfaceOps sherpa_surface = {
    .list_commands = sherpa_list_commands,
    .invoke_command = sherpa_invoke_command,
};

/* ── Engine vtable ─────────────────────────────────────────────────────── */

static TypioResult sherpa_init(TypioEngine *engine, TypioInstance *instance) {
    SherpaState *state = calloc(1, sizeof(SherpaState));
    if (!state) {
        return TYPIO_ERROR_OUT_OF_MEMORY;
    }
    state->instance = instance;
    typio_engine_set_user_data(engine, state);
    typio_engine_set_surface_ops(engine, &sherpa_surface);

    if (instance) {
        load_model(state, instance);
    }

    return TYPIO_OK;
}

static void sherpa_destroy(TypioEngine *engine) {
    SherpaState *state = (SherpaState *)typio_engine_get_user_data(engine);
    if (state) {
        unload_model(state);
        free(state);
        typio_engine_set_user_data(engine, NULL);
    }
}

static void sherpa_deactivate(TypioEngine *engine) {
    SherpaState *state = (SherpaState *)typio_engine_get_user_data(engine);
    if (state) {
        unload_model(state);
        typio_log_info("sherpa-onnx: model freed on deactivate");
    }
}

static void sherpa_focus_in(TypioEngine *engine, TypioInputContext *ctx) {
    (void)ctx;
    SherpaState *state = (SherpaState *)typio_engine_get_user_data(engine);
    if (!state || state->recognizer) {
        return;
    }
    if (load_model(state, engine->instance)) {
        typio_log_info("sherpa-onnx: model loaded");
    } else {
        typio_log_warning("sherpa-onnx: failed to load model");
    }
}

static void sherpa_focus_out(TypioEngine *engine, TypioInputContext *ctx) {
    (void)engine;
    (void)ctx;
}

static void sherpa_reset(TypioEngine *engine, TypioInputContext *ctx) {
    (void)engine;
    (void)ctx;
}

static TypioResult sherpa_reload_config(TypioEngine *engine) {
    SherpaState *state = (SherpaState *)typio_engine_get_user_data(engine);
    if (state) {
        unload_model(state);
    }
    return TYPIO_OK;
}

static TypioEngineAvailability sherpa_availability(TypioEngine *engine) {
    SherpaState *state = (SherpaState *)typio_engine_get_user_data(engine);
    return (state && state->recognizer)
        ? TYPIO_ENGINE_READY
        : TYPIO_ENGINE_PREPARING;
}

static char *sherpa_process_audio(TypioVoiceEngine *engine,
                                   const float *samples, size_t n_samples) {
    SherpaState *state = (SherpaState *)typio_engine_get_user_data(&engine->base);
    if (!state || !state->recognizer) {
        return NULL;
    }

    const SherpaOnnxOfflineStream *stream =
        SherpaOnnxCreateOfflineStream(state->recognizer);
    if (!stream) {
        typio_log_error("sherpa-onnx: failed to create stream");
        return NULL;
    }

    SherpaOnnxAcceptWaveformOffline(stream, 16000, samples, (int32_t)n_samples);
    SherpaOnnxDecodeOfflineStream(state->recognizer, stream);

    const SherpaOnnxOfflineRecognizerResult *r =
        SherpaOnnxGetOfflineStreamResult(stream);

    char *result = NULL;
    if (r && r->text && r->text[0] != '\0') {
        result = strdup(r->text);
    }

    if (r) {
        SherpaOnnxDestroyOfflineRecognizerResult(r);
    }
    SherpaOnnxDestroyOfflineStream(stream);

    return result;
}

/* ── Static tables ─────────────────────────────────────────────────────── */

static TypioEngineBaseOps sherpa_base_ops = {
    .init = sherpa_init,
    .destroy = sherpa_destroy,
    .deactivate = sherpa_deactivate,
    .focus_in = sherpa_focus_in,
    .focus_out = sherpa_focus_out,
    .reset = sherpa_reset,
    .reload_config = sherpa_reload_config,
    .availability = sherpa_availability,
};

static TypioVoiceEngineOps sherpa_voice_ops = {
    .process_audio = sherpa_process_audio,
};

static const char *const sherpa_required_caps[] = {
    "voice_input",
    NULL,
};

static TypioEngineInfo sherpa_info = {
    .name = "sherpa-onnx",
    .display_name = "Sherpa-ONNX",
    .description = "Speech-to-text via sherpa-onnx",
    .author = "Typio",
    .icon = "typio-sherpa",
    .language = "und",
    .type = TYPIO_ENGINE_TYPE_VOICE,
    .required_capabilities = sherpa_required_caps,
    .optional_capabilities = NULL,
};

static TypioVoiceEngine *sherpa_create(void) {
    return typio_voice_engine_new(&sherpa_info, &sherpa_base_ops, &sherpa_voice_ops);
}

TYPIO_VOICE_ENGINE_DEFINE(sherpa_info, sherpa_create)
