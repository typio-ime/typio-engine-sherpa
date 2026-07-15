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
#include <openssl/evp.h>
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
    pthread_mutex_t setup_lock;
    pthread_t setup_thread;
    bool setup_thread_valid;
    bool setup_running;
    bool setup_cancel;
    bool setup_model_ready;
} SherpaState;

/* ── Helpers ───────────────────────────────────────────────────────────── */

static bool file_exists(const char *path) {
    struct stat st;
    return stat(path, &st) == 0 && S_ISREG(st.st_mode);
}

static bool join_path(char *buf, size_t buf_size,
                      const char *dir, const char *name) {
    if (!buf || buf_size == 0 || !dir || !name) {
        return false;
    }
    int written = snprintf(buf, buf_size, "%s/%s", dir, name);
    if (written < 0 || (size_t)written >= buf_size) {
        buf[0] = '\0';
        return false;
    }
    return true;
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
            if (!join_path(buf, buf_size, dir, ent->d_name)) {
                closedir(d);
                return false;
            }
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
    if (!join_path(buf, buf_size, dir, exact)) {
        return false;
    }
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
        if (!join_path(model_dir, dir_size, base_dir, ent->d_name)) {
            continue;
        }
        struct stat st;
        if (lstat(model_dir, &st) != 0 || !S_ISDIR(st.st_mode)) {
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
    if (!join_path(base_dir, sizeof(base_dir), data_dir, "sherpa-onnx")) {
        typio_log_warning("sherpa-onnx: data directory path is too long");
        return false;
    }

    char model_dir[512];
    char tokens_path[512] = {};
    char file1[512] = {};
    char file2[512] = {};
    char file3[512] = {};
    SherpaModelType type = SHERPA_MODEL_UNKNOWN;

    if (model_hint && *model_hint) {
        if (join_path(model_dir, sizeof(model_dir), base_dir, model_hint)) {
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
    return true;
}

static bool ensure_model_loaded(TypioEngine *engine, const char *reason) {
    const char *why = reason ? reason : "unspecified";
    if (!engine || !engine->instance) {
        typio_log_warning("sherpa-onnx: cannot load model: engine not initialized");
        return false;
    }
    SherpaState *state = (SherpaState *)typio_engine_get_user_data(engine);
    if (!state) {
        typio_log_warning("sherpa-onnx: cannot load model: engine has no state");
        return false;
    }
    if (state->recognizer) {
        return true;
    }
    if (load_model(state, engine->instance)) {
        typio_log_info("sherpa-onnx: model loaded (%s)", why);
        return true;
    }
    typio_log_warning("sherpa-onnx: failed to load model (%s)", why);
    return false;
}

/* ── Model download ───────────────────────────────────────────────────── */

#define SHERPA_DEFAULT_MODEL_VERSION "2024-07-17"
#define SHERPA_MODEL_BASE_URL \
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models"

typedef struct {
    const char *name;
    const char *url_suffix;
    const char *sha256;
    const char *files[4];
} SherpaInstallableModel;

static const SherpaInstallableModel sherpa_installable_models[] = {
    {
        .name = "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-" SHERPA_DEFAULT_MODEL_VERSION,
        .url_suffix = "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-"
                      SHERPA_DEFAULT_MODEL_VERSION ".tar.bz2",
        .sha256 = "7d1efa2138a65b0b488df37f8b89e3d91a60676e416f515b952358d83dfd347e",
        .files = {"model.int8.onnx", "tokens.txt", NULL},
    },
};

static bool setup_cancelled(SherpaState *state) {
    pthread_mutex_lock(&state->setup_lock);
    bool cancelled = state->setup_cancel;
    pthread_mutex_unlock(&state->setup_lock);
    return cancelled;
}

static int download_progress(void *ud,
                             curl_off_t download_total,
                             curl_off_t download_now,
                             curl_off_t upload_total,
                             curl_off_t upload_now) {
    (void)download_total;
    (void)download_now;
    (void)upload_total;
    (void)upload_now;
    return setup_cancelled(ud) ? 1 : 0;
}

static size_t write_cb(void *ptr, size_t size, size_t nmemb, void *ud) {
    int fd = *(int *)ud;
    if (size != 0 && nmemb > SIZE_MAX / size) {
        return 0;
    }
    size_t total = size * nmemb;
    size_t offset = 0;
    while (offset < total) {
        ssize_t written = write(fd, (const char *)ptr + offset, total - offset);
        if (written < 0) {
            if (errno == EINTR) {
                continue;
            }
            return offset;
        }
        if (written == 0) {
            return offset;
        }
        offset += (size_t)written;
    }
    return total;
}

static bool sha256_matches(const char *path, const char *expected) {
    static const char hex[] = "0123456789abcdef";
    unsigned char buffer[64 * 1024];
    unsigned char digest[EVP_MAX_MD_SIZE];
    unsigned int digest_len = 0;
    bool ok = false;

    int fd = open(path, O_RDONLY | O_CLOEXEC);
    EVP_MD_CTX *ctx = EVP_MD_CTX_new();
    if (fd < 0 || !ctx || EVP_DigestInit_ex(ctx, EVP_sha256(), NULL) != 1) {
        if (fd >= 0) {
            close(fd);
        }
        EVP_MD_CTX_free(ctx);
        return false;
    }

    for (;;) {
        ssize_t count = read(fd, buffer, sizeof(buffer));
        if (count > 0) {
            if (EVP_DigestUpdate(ctx, buffer, (size_t)count) != 1) {
                break;
            }
            continue;
        }
        if (count < 0 && errno == EINTR) {
            continue;
        }
        if (count == 0 &&
            EVP_DigestFinal_ex(ctx, digest, &digest_len) == 1 &&
            digest_len == 32) {
            char actual[65];
            for (unsigned int i = 0; i < digest_len; i++) {
                actual[i * 2] = hex[digest[i] >> 4];
                actual[i * 2 + 1] = hex[digest[i] & 0x0f];
            }
            actual[64] = '\0';
            ok = strcmp(actual, expected) == 0;
        }
        break;
    }

    close(fd);
    EVP_MD_CTX_free(ctx);
    return ok;
}

static TypioResult download_to_tmp(SherpaState *state,
                                   const char *url,
                                   char *tmp_path,
                                   size_t path_size) {
    static const char path_template[] = "/tmp/typio-model-download-XXXXXX";
    if (path_size < sizeof(path_template)) {
        return TYPIO_ERROR_INVALID_ARGUMENT;
    }
    memcpy(tmp_path, path_template, sizeof(path_template));
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
    curl_easy_setopt(curl, CURLOPT_PROTOCOLS_STR, "https");
    curl_easy_setopt(curl, CURLOPT_REDIR_PROTOCOLS_STR, "https");
    curl_easy_setopt(curl, CURLOPT_USERAGENT, "typio-engine-sherpa/0.2.2");
    curl_easy_setopt(curl, CURLOPT_CONNECTTIMEOUT, 30L);
    curl_easy_setopt(curl, CURLOPT_TIMEOUT, 900L);
    curl_easy_setopt(curl, CURLOPT_LOW_SPEED_LIMIT, 1024L);
    curl_easy_setopt(curl, CURLOPT_LOW_SPEED_TIME, 60L);
    curl_easy_setopt(curl, CURLOPT_NOPROGRESS, 0L);
    curl_easy_setopt(curl, CURLOPT_XFERINFOFUNCTION, download_progress);
    curl_easy_setopt(curl, CURLOPT_XFERINFODATA, state);

    CURLcode res = curl_easy_perform(curl);
    curl_easy_cleanup(curl);
    close(fd);

    if (res != CURLE_OK) {
        unlink(tmp_path);
        return TYPIO_ERROR;
    }
    return TYPIO_OK;
}

static bool ensure_directory(const char *path) {
    if (mkdir(path, 0755) == 0) {
        return true;
    }
    if (errno != EEXIST) {
        return false;
    }
    struct stat st;
    return lstat(path, &st) == 0 && S_ISDIR(st.st_mode);
}

static bool wait_child_success(pid_t pid) {
    int status = 0;
    pid_t result;
    do {
        result = waitpid(pid, &status, 0);
    } while (result < 0 && errno == EINTR);
    return result == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0;
}

static void remove_tree(const char *path) {
    pid_t pid = fork();
    if (pid == 0) {
        execlp("rm", "rm", "-rf", "--", path, (char *)NULL);
        _exit(127);
    }
    if (pid > 0) {
        (void)wait_child_success(pid);
    }
}

static TypioResult extract_files(const char *tar_path,
                                  const char *dest_dir,
                                  const char *const *files) {
    char extract_root[] = "/tmp/typio-model-extract-XXXXXX";
    if (!mkdtemp(extract_root)) {
        return TYPIO_ERROR;
    }

    pid_t pid = fork();
    if (pid < 0) {
        remove_tree(extract_root);
        return TYPIO_ERROR;
    }
    if (pid == 0) {
        execlp("tar", "tar", "xf", tar_path,
               "-C", extract_root,
               "--no-same-owner", "--no-same-permissions",
               (char *)NULL);
        _exit(127);
    }
    if (!wait_child_success(pid)) {
        remove_tree(extract_root);
        return TYPIO_ERROR;
    }

    char src_dir[512];
    bool found = false;
    DIR *d = opendir(extract_root);
    if (d) {
        static const char model_prefix[] = "sherpa-onnx-sense-voice";
        struct dirent *ent;
        while ((ent = readdir(d))) {
            if (strncmp(ent->d_name, model_prefix, sizeof(model_prefix) - 1) == 0 &&
                join_path(src_dir, sizeof(src_dir), extract_root, ent->d_name)) {
                struct stat st;
                if (lstat(src_dir, &st) == 0 && S_ISDIR(st.st_mode)) {
                    found = true;
                    break;
                }
            }
        }
        closedir(d);
    }
    if (!found) {
        remove_tree(extract_root);
        return TYPIO_ERROR;
    }

    for (size_t i = 0; files[i]; i++) {
        char src[1024], dst[1024];
        if (!join_path(src, sizeof(src), src_dir, files[i]) ||
            !join_path(dst, sizeof(dst), dest_dir, files[i])) {
            remove_tree(extract_root);
            return TYPIO_ERROR;
        }
        struct stat st;
        if (lstat(src, &st) != 0 || !S_ISREG(st.st_mode)) {
            remove_tree(extract_root);
            return TYPIO_ERROR;
        }

        pid_t cp = fork();
        if (cp < 0) {
            remove_tree(extract_root);
            return TYPIO_ERROR;
        }
        if (cp == 0) {
            execlp("cp", "cp", "--", src, dst, (char *)NULL);
            _exit(127);
        }
        if (!wait_child_success(cp)) {
            remove_tree(extract_root);
            return TYPIO_ERROR;
        }
    }

    remove_tree(extract_root);
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

    char model_root[512];
    char dest_dir[512];
    if (!join_path(model_root, sizeof(model_root), data_dir, "sherpa-onnx") ||
        !join_path(dest_dir, sizeof(dest_dir), model_root, model->name)) {
        typio_log_error("sherpa-onnx: model destination path is too long");
        return TYPIO_ERROR_INVALID_ARGUMENT;
    }

    struct stat st;
    if (lstat(dest_dir, &st) == 0 && S_ISDIR(st.st_mode)) {
        bool complete = true;
        for (size_t i = 0; model->files[i]; i++) {
            char path[1024];
            if (!join_path(path, sizeof(path), dest_dir, model->files[i]) ||
                !file_exists(path)) {
                complete = false;
                break;
            }
        }
        if (complete) {
            typio_log_info("sherpa-onnx: model '%s' already installed at %s",
                           model->name, dest_dir);
            return TYPIO_OK;
        }
        typio_log_warning("sherpa-onnx: repairing incomplete model at %s",
                          dest_dir);
    }

    if (!ensure_directory(model_root) || !ensure_directory(dest_dir)) {
        typio_log_error("sherpa-onnx: cannot create model directory %s", dest_dir);
        return TYPIO_ERROR;
    }

    char url[1024];
    if (!join_path(url, sizeof(url), SHERPA_MODEL_BASE_URL, model->url_suffix)) {
        return TYPIO_ERROR_INVALID_ARGUMENT;
    }

    typio_log_info("sherpa-onnx: downloading %s", model->url_suffix);

    char tmp_path[512];
    TypioResult res = download_to_tmp(state, url, tmp_path, sizeof(tmp_path));
    if (res != TYPIO_OK) {
        if (setup_cancelled(state)) {
            typio_log_info("sherpa-onnx: download cancelled");
        } else {
            typio_log_error("sherpa-onnx: download failed");
        }
        return res;
    }

    if (!sha256_matches(tmp_path, model->sha256)) {
        unlink(tmp_path);
        typio_log_error("sherpa-onnx: downloaded model checksum mismatch");
        return TYPIO_ERROR;
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
    SherpaState *state = (SherpaState *)typio_engine_get_user_data(job->engine);
    TypioResult res = install_default_model(job->engine);
    if (state) {
        pthread_mutex_lock(&state->setup_lock);
        state->setup_model_ready = res == TYPIO_OK;
        state->setup_running = false;
        pthread_mutex_unlock(&state->setup_lock);
    }
    free(job);
    return NULL;
}

static const TypioEngineCommand sherpa_commands[] = {
    {.id = "setup", .label = "Download and verify default model"},
};

static const TypioEngineCommand *sherpa_list_commands(TypioEngine *engine,
                                                       size_t *out_count) {
    (void)engine;
    *out_count = sizeof(sherpa_commands) / sizeof(sherpa_commands[0]);
    return sherpa_commands;
}

static TypioResult sherpa_invoke_command(TypioEngine *engine, const char *id) {
    if (strcmp(id, "setup") == 0) {
        SherpaState *state = (SherpaState *)typio_engine_get_user_data(engine);
        if (!state) {
            return TYPIO_ERROR_NOT_INITIALIZED;
        }

        pthread_mutex_lock(&state->setup_lock);
        if (state->setup_running) {
            pthread_mutex_unlock(&state->setup_lock);
            typio_log_info("sherpa-onnx: setup is already running");
            return TYPIO_OK;
        }
        if (state->setup_thread_valid) {
            pthread_t completed = state->setup_thread;
            state->setup_thread_valid = false;
            pthread_mutex_unlock(&state->setup_lock);
            (void)pthread_join(completed, NULL);
            pthread_mutex_lock(&state->setup_lock);
        }

        SherpaSetupJob *job = calloc(1, sizeof(SherpaSetupJob));
        if (!job) {
            pthread_mutex_unlock(&state->setup_lock);
            return TYPIO_ERROR_OUT_OF_MEMORY;
        }
        job->engine = engine;
        state->setup_cancel = false;
        state->setup_model_ready = false;
        state->setup_running = true;
        if (pthread_create(&state->setup_thread, NULL,
                           sherpa_setup_thread, job) != 0) {
            state->setup_running = false;
            pthread_mutex_unlock(&state->setup_lock);
            free(job);
            return TYPIO_ERROR;
        }
        state->setup_thread_valid = true;
        pthread_mutex_unlock(&state->setup_lock);
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
    if (pthread_mutex_init(&state->setup_lock, NULL) != 0) {
        free(state);
        return TYPIO_ERROR;
    }
    if (curl_global_init(CURL_GLOBAL_DEFAULT) != CURLE_OK) {
        pthread_mutex_destroy(&state->setup_lock);
        free(state);
        return TYPIO_ERROR;
    }
    state->instance = instance;
    typio_engine_set_user_data(engine, state);
    typio_engine_set_surface_ops(engine, &sherpa_surface);

    (void)ensure_model_loaded(engine, "init");

    return TYPIO_OK;
}

static void sherpa_destroy(TypioEngine *engine) {
    SherpaState *state = (SherpaState *)typio_engine_get_user_data(engine);
    if (state) {
        pthread_mutex_lock(&state->setup_lock);
        bool join_setup = state->setup_thread_valid;
        pthread_t setup_thread = state->setup_thread;
        state->setup_cancel = true;
        state->setup_thread_valid = false;
        pthread_mutex_unlock(&state->setup_lock);
        if (join_setup) {
            (void)pthread_join(setup_thread, NULL);
        }
        unload_model(state);
        curl_global_cleanup();
        pthread_mutex_destroy(&state->setup_lock);
        free(state);
        typio_engine_set_user_data(engine, NULL);
    }
}



static void sherpa_focus_in(TypioEngine *engine, TypioInputContext *ctx) {
    (void)ctx;
    (void)ensure_model_loaded(engine, "focus-in");
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
        (void)ensure_model_loaded(engine, "config reload");
    }
    return TYPIO_OK;
}

/* Per-key live reaction is unnecessary for sherpa-onnx: the only configured
 * values are model paths, which reload_config covers via a full reload. */
static void sherpa_on_config_change(TypioEngine *engine,
                                     const char *key,
                                     const char *value) {
    (void)engine;
    (void)key;
    (void)value;
}

static TypioEngineAvailability sherpa_availability(TypioEngine *engine) {
    SherpaState *state = (SherpaState *)typio_engine_get_user_data(engine);
    if (state && !state->recognizer) {
        pthread_mutex_lock(&state->setup_lock);
        bool load_installed_model = state->setup_model_ready;
        state->setup_model_ready = false;
        pthread_mutex_unlock(&state->setup_lock);
        if (load_installed_model) {
            (void)ensure_model_loaded(engine, "setup completed");
        }
    }
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
    .focus_in = sherpa_focus_in,
    .focus_out = sherpa_focus_out,
    .reset = sherpa_reset,
    .reload_config = sherpa_reload_config,
    .on_config_change = sherpa_on_config_change,
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
    .description = "Voice input engine powered by sherpa-onnx.",
    .author = "Typio",
    .icon = "typio-sherpa-symbolic",
    .language = "und",
    .type = TYPIO_ENGINE_TYPE_VOICE,
    .required_capabilities = sherpa_required_caps,
    .optional_capabilities = NULL,
};

static TypioVoiceEngine *sherpa_create(void) {
    return typio_voice_engine_new(&sherpa_info, &sherpa_base_ops, &sherpa_voice_ops);
}

TYPIO_VOICE_ENGINE_DEFINE(sherpa_info, sherpa_create)
