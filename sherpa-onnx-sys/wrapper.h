/* Minimal wrapper for bindgen.
 *
 * Only the offline recognizer surface is bound; see build.rs for the
 * allowlist. Binding the full header would drag in every model family and
 * turn each upstream struct addition into churn. */
#include <sherpa-onnx/c-api/c-api.h>
