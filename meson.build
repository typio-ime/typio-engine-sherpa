project('typio-engine-sherpa', 'c',
    version: '0.2.0',
    default_options: [
        'c_std=c23',
        'warning_level=2',
        'werror=false',
    ],
    meson_version: '>=1.0.0',
)

cc = meson.get_compiler('c')

add_project_arguments('-D_POSIX_C_SOURCE=200809L', language: 'c')

# Engine ABI from libtypio
typio_abi = dependency('typio-engine-abi', required: true)

# Sherpa-ONNX library
sherpa_onnx_dep = dependency('sherpa-onnx', required: false)
if not sherpa_onnx_dep.found()
    sherpa_onnx_lib = cc.find_library('sherpa-onnx-c-api', required: true)
    sherpa_onnx_dep = declare_dependency(dependencies: sherpa_onnx_lib)
endif

sherpa_src = files('src/sherpa.c', 'src/worker_main.c')

curl_dep = dependency('libcurl', required: true)

worker_install_dir = get_option('libexecdir') / 'typio' / 'engines'
manifest_install_dir = get_option('datadir') / 'typio' / 'engines'

sherpa_worker = executable('typio-engine-sherpa',
    sherpa_src,
    dependencies: [typio_abi, sherpa_onnx_dep, curl_dep],
    install: true,
    install_dir: worker_install_dir,
)

dev_manifest_conf = configuration_data()
dev_manifest_conf.set('ENGINE_COMMAND', './typio-engine-sherpa')
configure_file(
    input: 'typio-engine-sherpa.toml.in',
    output: 'typio-engine-sherpa.toml',
    configuration: dev_manifest_conf,
)

install_manifest_conf = configuration_data()
install_manifest_conf.set(
    'ENGINE_COMMAND',
    get_option('prefix') / worker_install_dir / 'typio-engine-sherpa',
)
installed_manifest = configure_file(
    input: 'typio-engine-sherpa.toml.in',
    output: 'typio-engine-sherpa.installed.toml',
    configuration: install_manifest_conf,
)
install_data(installed_manifest,
    install_dir: manifest_install_dir,
    rename: 'typio-engine-sherpa.toml',
)

# Install freedesktop icons so hosts can resolve "typio-sherpa" via the icon theme.
icon_dir = get_option('datadir') / 'icons' / 'hicolor' / 'scalable' / 'apps'
install_data(
    'data/icons/hicolor/scalable/apps/typio-sherpa-symbolic.svg',
    install_dir: icon_dir,
)
