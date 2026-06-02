project('typio-engine-sherpa', 'c',
    version: '0.0.3',
    default_options: [
        'c_std=c23',
        'warning_level=2',
        'werror=false',
    ],
    meson_version: '>=1.0.0',
)

cc = meson.get_compiler('c')

# Engine ABI from libtypio
typio_abi = dependency('typio-engine-abi', required: true)

# Sherpa-ONNX library
sherpa_onnx_dep = dependency('sherpa-onnx', required: false)
if not sherpa_onnx_dep.found()
    sherpa_onnx_lib = cc.find_library('sherpa-onnx-c-api', required: true)
    sherpa_onnx_dep = declare_dependency(dependencies: sherpa_onnx_lib)
endif

sherpa_src = files('src/sherpa.c')

curl_dep = dependency('libcurl', required: true)

engine_dir = get_option('libdir') / 'typio' / 'engines'

shared_library('typio_engine_sherpa',
    sherpa_src,
    dependencies: [typio_abi, sherpa_onnx_dep, curl_dep],
    install: true,
    install_dir: engine_dir,
)

# Install freedesktop icons so hosts can resolve "typio-sherpa" via the icon theme.
icon_dir = get_option('datadir') / 'icons' / 'hicolor' / 'scalable' / 'apps'
install_data(
    'data/icons/hicolor/scalable/apps/typio-sherpa-symbolic.svg',
    install_dir: icon_dir,
)
