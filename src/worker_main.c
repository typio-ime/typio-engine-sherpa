#include "typio/abi/abi.h"
#include "typio/abi/engine_protocol.h"
#include "typio/runtime/instance.h"
#include "typio/schema/config_schema.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static FILE *typio_engine_response_out;
#undef stdout
#define stdout typio_engine_response_out

#if defined(__GNUC__) || defined(__clang__)
#define TYPIO_WEAK __attribute__((weak))
#else
#define TYPIO_WEAK
#endif

TYPIO__EXTERN_C const TypioEngineInfo *typio_engine_get_info(void);
TYPIO__EXTERN_C TypioKeyboardEngine *typio_keyboard_engine_create(void) TYPIO_WEAK;
TYPIO__EXTERN_C TypioVoiceEngine *typio_voice_engine_create(void) TYPIO_WEAK;
TYPIO__EXTERN_C const TypioConfigField *
typio_engine_get_config_schema(size_t *out_count) TYPIO_WEAK;

typedef struct {
    TypioInstance *instance;
    TypioInputContext *ctx;
    const TypioEngineInfo *info;
    TypioKeyboardEngine *keyboard;
    TypioVoiceEngine *voice;
    TypioEngine *base;
} Worker;

static void write_hex(FILE *out, const char *text) {
    static const char hex[] = "0123456789abcdef";
    if (!text) {
        return;
    }
    const unsigned char *p = (const unsigned char *)text;
    while (*p) {
        fputc(hex[*p >> 4], out);
        fputc(hex[*p & 0x0f], out);
        p++;
    }
}

static unsigned hex_value(char c) {
    if (c >= '0' && c <= '9') return (unsigned)(c - '0');
    if (c >= 'a' && c <= 'f') return (unsigned)(c - 'a' + 10);
    if (c >= 'A' && c <= 'F') return (unsigned)(c - 'A' + 10);
    return 0xffu;
}

static unsigned char *decode_hex_bytes(const char *hex, size_t *out_len) {
    size_t len = strlen(hex);
    if ((len % 2) != 0) {
        return NULL;
    }
    unsigned char *bytes = malloc(len / 2);
    if (!bytes) {
        return NULL;
    }
    for (size_t i = 0; i < len; i += 2) {
        unsigned hi = hex_value(hex[i]);
        unsigned lo = hex_value(hex[i + 1]);
        if (hi > 0xf || lo > 0xf) {
            free(bytes);
            return NULL;
        }
        bytes[i / 2] = (unsigned char)((hi << 4) | lo);
    }
    *out_len = len / 2;
    return bytes;
}

static char *decode_hex_string(const char *hex) {
    size_t byte_len = 0;
    unsigned char *bytes = decode_hex_bytes(hex ? hex : "", &byte_len);
    if (!bytes && byte_len == 0 && hex && *hex) {
        return NULL;
    }
    char *text = malloc(byte_len + 1);
    if (!text) {
        free(bytes);
        return NULL;
    }
    if (byte_len > 0) {
        memcpy(text, bytes, byte_len);
    }
    text[byte_len] = '\0';
    free(bytes);
    return text;
}

static void write_hex_field(FILE *out, const char *text) {
    if (text) {
        write_hex(out, text);
    }
}

static void write_mode_line(const char *prefix,
                            const TypioKeyboardEngineMode *mode,
                            bool active) {
    if (!mode || !mode->id) {
        return;
    }
    fputs(prefix, stdout);
    fputc('\t', stdout);
    write_hex_field(stdout, mode->id);
    fputc('\t', stdout);
    write_hex_field(stdout, mode->label);
    fputc('\t', stdout);
    write_hex_field(stdout, mode->display_label);
    fputc('\t', stdout);
    write_hex_field(stdout, mode->icon_name);
    fputc('\t', stdout);
    write_hex_field(stdout, mode->profile_id);
    fputc('\t', stdout);
    write_hex_field(stdout, mode->profile_label);
    fputc('\t', stdout);
    write_hex_field(stdout, mode->description);
    /* is_active, then salience (0=quiet, 1=notable) so the host can honour the
     * engine's on-focus auto-reveal intent. */
    fprintf(stdout, "\t%d\t%d\n", active ? 1 : 0, (int)mode->salience);
}

static void commit_cb(TypioInputContext *ctx, const char *text, void *user_data) {
    (void)ctx;
    (void)user_data;
    FILE *out = stdout;
    fputs("COMMIT\t", out);
    write_hex(out, text);
    fputc('\n', out);
}

static void composition_cb(TypioInputContext *ctx,
                           const TypioComposition *comp,
                           void *user_data) {
    (void)ctx;
    (void)user_data;
    FILE *out = stdout;
    if (!comp) {
        fputs("CLEAR\n", out);
        return;
    }
    fprintf(out, "COMPOSITION\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%u\t",
            comp->cursor_pos,
            comp->page,
            comp->page_size,
            comp->total,
            comp->selected,
            comp->has_prev ? 1 : 0,
            comp->has_next ? 1 : 0,
            comp->host_managed_selection);
    for (size_t i = 0; i < comp->segment_count; i++) {
        if (i) fputc(',', out);
        write_hex(out, comp->segments[i].text);
    }
    fputc('\t', out);
    for (size_t i = 0; i < comp->candidate_count; i++) {
        if (i) fputc(',', out);
        write_hex(out, comp->candidates[i].text);
    }
    fputc('\n', out);
}

static bool worker_init(Worker *worker) {
    if (typio_engine_get_config_schema) {
        size_t count = 0;
        const TypioConfigField *fields = typio_engine_get_config_schema(&count);
        if (fields && count > 0) {
            typio_config_schema_register_many(fields, count);
        }
    }

    worker->info = typio_engine_get_info();
    if (!worker->info) {
        fprintf(stderr, "typio-engine: engine returned null info\n");
        return false;
    }

    if (worker->info->type == TYPIO_ENGINE_TYPE_VOICE) {
        if (!typio_voice_engine_create) {
            fprintf(stderr, "typio-engine: missing voice factory\n");
            return false;
        }
        worker->voice = typio_voice_engine_create();
        worker->base = worker->voice ? &worker->voice->base : NULL;
    } else {
        if (!typio_keyboard_engine_create) {
            fprintf(stderr, "typio-engine: missing keyboard factory\n");
            return false;
        }
        worker->keyboard = typio_keyboard_engine_create();
        worker->base = worker->keyboard ? &worker->keyboard->base : NULL;
    }
    if (!worker->base || !worker->base->base_ops) {
        fprintf(stderr, "typio-engine: factory returned invalid engine\n");
        return false;
    }

    worker->instance = typio_instance_new();
    if (!worker->instance || typio_instance_init(worker->instance) != TYPIO_OK) {
        fprintf(stderr, "typio-engine: failed to create local instance\n");
        return false;
    }
    worker->ctx = typio_instance_create_context(worker->instance);
    if (!worker->ctx) {
        fprintf(stderr, "typio-engine: failed to create local context\n");
        return false;
    }
    typio_input_context_set_commit_callback(worker->ctx, commit_cb, stdout);
    typio_input_context_set_composition_callback(worker->ctx, composition_cb, stdout);

    worker->base->instance = worker->instance;
    if (worker->base->base_ops->init) {
        TypioResult result = worker->base->base_ops->init(worker->base,
                                                          worker->instance);
        if (result != TYPIO_OK) {
            fprintf(stderr, "typio-engine: engine init failed: %d\n", result);
            return false;
        }
    }
    worker->base->initialized = true;
    return true;
}

/* Teardown runs active → deactivated → destroyed.
 *
 * 1. deactivate(): let the engine stop any background work and detach its
 *    callbacks while everything is still alive (e.g. an async deployer or
 *    model-loading thread can't notify into a half-freed instance).
 * 2. free the instance: dropping its input contexts runs each context's
 *    session destructor, which dereferences the still-live engine state.
 * 3. free the engine: finalise the backend and release engine state.
 *
 * Freeing the engine before the instance would finalise the backend and
 * free the state out from under the session destructors — a use-after-free
 * crash on exit. */
static void worker_destroy(Worker *worker) {
    if (worker->base && worker->base->base_ops->deactivate) {
        worker->base->base_ops->deactivate(worker->base);
    }
    if (worker->instance) {
        typio_instance_free(worker->instance);
        worker->instance = NULL;
    }
    if (worker->base) {
        typio_engine_free(worker->base);
        worker->base = NULL;
    }
}

static char *field_next(char **save) {
    return strtok_r(NULL, "\t", save);
}

static uint32_t parse_u32(const char *s) {
    return s ? (uint32_t)strtoul(s, NULL, 10) : 0;
}

static uint64_t parse_u64(const char *s) {
    return s ? (uint64_t)strtoull(s, NULL, 10) : 0;
}

static void write_key_result(TypioKeyProcessResult result) {
    switch (result) {
    case TYPIO_KEY_HANDLED:
        fputs("RESULT\tHANDLED\n", stdout);
        break;
    case TYPIO_KEY_COMPOSING:
        fputs("RESULT\tCOMPOSING\n", stdout);
        break;
    case TYPIO_KEY_COMMITTED:
        fputs("RESULT\tCOMMITTED\n", stdout);
        break;
    case TYPIO_KEY_NOT_HANDLED:
    default:
        fputs("RESULT\tNOT_HANDLED\n", stdout);
        break;
    }
}

static void write_availability(TypioEngineAvailability availability) {
    switch (availability) {
    case TYPIO_ENGINE_UNINITIALIZED:
        fputs("AVAILABILITY\tUNINITIALIZED\n", stdout);
        break;
    case TYPIO_ENGINE_PREPARING:
        fputs("AVAILABILITY\tPREPARING\n", stdout);
        break;
    case TYPIO_ENGINE_FAILED:
        fputs("AVAILABILITY\tFAILED\n", stdout);
        break;
    case TYPIO_ENGINE_READY:
    default:
        fputs("AVAILABILITY\tREADY\n", stdout);
        break;
    }
}

static void handle_process_key(Worker *worker, char *save) {
    (void)field_next(&save);
    const char *state = field_next(&save);
    const char *keycode = field_next(&save);
    const char *keysym = field_next(&save);
    const char *modifiers = field_next(&save);
    const char *unicode = field_next(&save);
    const char *time = field_next(&save);
    const char *repeat = field_next(&save);
    const char *base_keysym = field_next(&save);

    if (!worker->keyboard || !worker->keyboard->keyboard ||
        !worker->keyboard->keyboard->process_key) {
        fputs("RESULT\tNOT_HANDLED\n", stdout);
        return;
    }

    TypioKeyEvent event = {
        .struct_size = sizeof(TypioKeyEvent),
        .type = (state && strcmp(state, "release") == 0)
            ? TYPIO_EVENT_KEY_RELEASE
            : TYPIO_EVENT_KEY_PRESS,
        .keycode = parse_u32(keycode),
        .keysym = parse_u32(keysym),
        .modifiers = parse_u32(modifiers),
        .unicode = parse_u32(unicode),
        .time = parse_u64(time),
        .is_repeat = repeat && strcmp(repeat, "1") == 0,
        .base_keysym = parse_u32(base_keysym),
    };
    write_key_result(worker->keyboard->keyboard->process_key(worker->keyboard,
                                                             worker->ctx,
                                                             &event));
}

static void handle_process_audio(Worker *worker, char *save) {
    const char *payload = field_next(&save);
    if (!worker->voice || !worker->voice->voice ||
        !worker->voice->voice->process_audio || !payload) {
        return;
    }
    size_t byte_len = 0;
    unsigned char *bytes = decode_hex_bytes(payload, &byte_len);
    if (!bytes || (byte_len % sizeof(float)) != 0) {
        free(bytes);
        return;
    }
    char *text = worker->voice->voice->process_audio(
        worker->voice, (const float *)bytes, byte_len / sizeof(float));
    free(bytes);
    if (text && *text) {
        fputs("TEXT\t", stdout);
        write_hex(stdout, text);
        fputc('\n', stdout);
    }
    free(text);
}

static void handle_list_modes(Worker *worker) {
    if (!worker->keyboard || !worker->keyboard->keyboard ||
        !worker->keyboard->keyboard->list_modes) {
        fputs("OK\n", stdout);
        return;
    }
    const TypioKeyboardEngineMode *active = NULL;
    if (worker->keyboard->keyboard->get_active_mode) {
        active = worker->keyboard->keyboard->get_active_mode(worker->keyboard,
                                                             worker->ctx);
    }
    size_t count = 0;
    const TypioKeyboardEngineMode *modes =
        worker->keyboard->keyboard->list_modes(worker->keyboard, &count);
    for (size_t i = 0; modes && i < count; i++) {
        bool is_active = active && active->id && modes[i].id &&
                         strcmp(active->id, modes[i].id) == 0;
        write_mode_line("MODE", &modes[i], is_active);
    }
}

static void handle_get_active_mode(Worker *worker) {
    if (!worker->keyboard || !worker->keyboard->keyboard ||
        !worker->keyboard->keyboard->get_active_mode) {
        fputs("OK\n", stdout);
        return;
    }
    const TypioKeyboardEngineMode *mode =
        worker->keyboard->keyboard->get_active_mode(worker->keyboard, worker->ctx);
    write_mode_line("ACTIVE_MODE", mode, true);
}

/* Append an ACTIVE_MODE line reporting the engine's current keyboard mode.
 *
 * A worker is host-driven and has no asynchronous channel, so the reply to any
 * request that may have flipped the internal mode is the only place to surface
 * it. The framework caches the reported mode and notifies the host's indicator
 * and tray on a transition. Emits nothing when the engine reports no mode;
 * never prints OK/RESULT, so it is safe to call after a branch's own reply. */
static void emit_active_mode(Worker *worker) {
    if (!worker->keyboard || !worker->keyboard->keyboard ||
        !worker->keyboard->keyboard->get_active_mode) {
        return;
    }
    const TypioKeyboardEngineMode *mode =
        worker->keyboard->keyboard->get_active_mode(worker->keyboard, worker->ctx);
    if (mode) {
        write_mode_line("ACTIVE_MODE", mode, true);
    }
}

static void handle_set_active_mode(Worker *worker, char *save) {
    (void)field_next(&save);
    const char *encoded_mode_id = field_next(&save);
    if (!worker->keyboard || !worker->keyboard->keyboard ||
        !worker->keyboard->keyboard->set_active_mode) {
        fputs("ERR\tset_active_mode unsupported\n", stdout);
        return;
    }
    char *mode_id = NULL;
    if (encoded_mode_id && *encoded_mode_id) {
        mode_id = decode_hex_string(encoded_mode_id);
        if (!mode_id) {
            fputs("ERR\tinvalid mode id\n", stdout);
            return;
        }
    }
    TypioResult result =
        worker->keyboard->keyboard->set_active_mode(worker->keyboard,
                                                    worker->ctx,
                                                    mode_id);
    free(mode_id);
    if (result == TYPIO_OK) {
        fputs("OK\n", stdout);
    } else {
        fprintf(stdout, "ERR\tset_active_mode failed: %d\n", result);
    }
}

static void handle_commit_candidate(Worker *worker, char *save) {
    (void)field_next(&save);
    const char *candidate_index = field_next(&save);
    if (!worker->keyboard || !worker->keyboard->keyboard ||
        !worker->keyboard->keyboard->commit_candidate) {
        fputs("ERR\tcommit_candidate unsupported\n", stdout);
        return;
    }
    TypioResult result =
        worker->keyboard->keyboard->commit_candidate(worker->keyboard,
                                                     worker->ctx,
                                                     (int)strtol(candidate_index ? candidate_index : "0",
                                                                 NULL,
                                                                 10));
    if (result == TYPIO_OK) {
        fputs("OK\n", stdout);
    } else {
        fprintf(stdout, "ERR\tcommit_candidate failed: %d\n", result);
    }
}

static bool handle_request(Worker *worker, char *line) {
    char *save = NULL;
    char *op = strtok_r(line, "\t", &save);
    if (!op) {
        fputs("END\n", stdout);
        fflush(stdout);
        return true;
    }
    if (strcmp(op, "shutdown") == 0) {
        return false;
    } else if (strcmp(op, "init") == 0 || strcmp(op, "reload-config") == 0) {
        if (strcmp(op, "reload-config") == 0 &&
            worker->base->base_ops->reload_config) {
            (void)worker->base->base_ops->reload_config(worker->base);
        }
        fputs("OK\n", stdout);
    } else if (strcmp(op, "deactivate") == 0) {
        if (worker->base->base_ops->deactivate) {
            worker->base->base_ops->deactivate(worker->base);
        }
        fputs("OK\n", stdout);
    } else if (strcmp(op, "focus-in") == 0) {
        if (worker->base->base_ops->focus_in) {
            worker->base->base_ops->focus_in(worker->base, worker->ctx);
        }
        fputs("OK\n", stdout);
        emit_active_mode(worker);
    } else if (strcmp(op, "focus-out") == 0) {
        if (worker->base->base_ops->focus_out) {
            worker->base->base_ops->focus_out(worker->base, worker->ctx);
        }
        fputs("OK\n", stdout);
    } else if (strcmp(op, "reset") == 0) {
        if (worker->base->base_ops->reset) {
            worker->base->base_ops->reset(worker->base, worker->ctx);
        }
        fputs("OK\n", stdout);
        emit_active_mode(worker);
    } else if (strcmp(op, "process-key") == 0) {
        handle_process_key(worker, save);
        emit_active_mode(worker);
    } else if (strcmp(op, "process-audio") == 0) {
        handle_process_audio(worker, save);
    } else if (strcmp(op, "list-modes") == 0) {
        handle_list_modes(worker);
    } else if (strcmp(op, "get-active-mode") == 0) {
        handle_get_active_mode(worker);
    } else if (strcmp(op, "set-active-mode") == 0) {
        handle_set_active_mode(worker, save);
        emit_active_mode(worker);
    } else if (strcmp(op, "commit-candidate") == 0) {
        handle_commit_candidate(worker, save);
    } else if (strcmp(op, "availability") == 0) {
        if (worker->base->base_ops->availability) {
            write_availability(worker->base->base_ops->availability(worker->base));
        } else {
            write_availability(TYPIO_ENGINE_READY);
        }
    } else {
        fprintf(stdout, "ERR\tunknown request: %s\n", op);
    }
    fputs("END\n", stdout);
    fflush(stdout);
    return true;
}

int main(void) {
    Worker worker = {0};
    int protocol_fd = TYPIO_ENGINE_PROTOCOL_FD;
    const char *fd_env = getenv("TYPIO_ENGINE_FD");
    if (fd_env && *fd_env) {
        protocol_fd = (int)strtol(fd_env, NULL, 10);
    }
    typio_engine_response_out = stderr;
    if (!worker_init(&worker)) {
        worker_destroy(&worker);
        return 1;
    }

    const char *engine_type =
        (worker.info && worker.info->type == TYPIO_ENGINE_TYPE_KEYBOARD)
            ? "keyboard"
            : "voice";
    if (!typio_engine_protocol_send_hello(protocol_fd,
                                     worker.info ? worker.info->name : "engine",
                                     engine_type)) {
        worker_destroy(&worker);
        return 1;
    }
    TypioEngineProtocolFrame host_hello;
    if (!typio_engine_protocol_read_frame(protocol_fd, &host_hello) ||
        host_hello.message_type != TYPIO_ENGINE_PROTOCOL_HOST_HELLO) {
        typio_engine_protocol_frame_free(&host_hello);
        worker_destroy(&worker);
        return 1;
    }
    typio_engine_protocol_frame_free(&host_hello);

    while (true) {
        TypioEngineProtocolFrame frame;
        if (!typio_engine_protocol_read_frame(protocol_fd, &frame)) {
            break;
        }
        if (frame.message_type != TYPIO_ENGINE_PROTOCOL_REQUEST || !frame.payload) {
            typio_engine_protocol_send_error(protocol_fd,
                                        frame.request_id,
                                        "expected engine protocol request");
            typio_engine_protocol_frame_free(&frame);
            continue;
        }

        char *line = (char *)frame.payload;
        if (strcmp(line, "shutdown") == 0) {
            typio_engine_protocol_frame_free(&frame);
            break;
        }

        char *response = NULL;
        size_t response_len = 0;
        FILE *mem = open_memstream(&response, &response_len);
        if (!mem) {
            typio_engine_protocol_send_error(protocol_fd,
                                        frame.request_id,
                                        "response buffer allocation failed");
            typio_engine_protocol_frame_free(&frame);
            continue;
        }
        typio_engine_response_out = mem;
        (void)handle_request(&worker, line);
        fclose(mem);
        typio_engine_response_out = stderr;
        typio_engine_protocol_send_response(protocol_fd,
                                       frame.request_id,
                                       response ? response : "",
                                       response_len);
        free(response);
        typio_engine_protocol_frame_free(&frame);
    }

    worker_destroy(&worker);
    return 0;
}
