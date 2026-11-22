cmake_minimum_required(VERSION 3.24)
project(esp32bench-ui VERSION 0.1.10 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_AUTOMOC ON)

option(BUILD_TESTING "Build the UI tests" ON)

find_package(Qt6 6.4 REQUIRED COMPONENTS Core Network Widgets LinguistTools)
if(BUILD_TESTING)
    find_package(Qt6 6.4 REQUIRED COMPONENTS Test)
endif()
qt_standard_project_setup()
include(GNUInstallDirs)

# Everything except main() goes in a library so the tests can link it.
add_library(esp32bench_ui STATIC
    src/RpcClient.cpp src/RpcClient.h
    src/BoardInfoDialog.cpp src/BoardInfoDialog.h
    src/BuildPanel.cpp src/BuildPanel.h
    src/BacktraceDialog.cpp src/BacktraceDialog.h
    src/CodeView.cpp src/CodeView.h
    src/SketchHighlighter.cpp src/SketchHighlighter.h
    src/ConsoleView.cpp src/ConsoleView.h
    src/CoreConnection.cpp src/CoreConnection.h
    src/DebugPanel.cpp src/DebugPanel.h
    src/DisplayPanel.cpp src/DisplayPanel.h
    src/PinsPanel.cpp src/PinsPanel.h
    src/PlotPanel.cpp src/PlotPanel.h
    src/CircuitPanel.cpp src/CircuitPanel.h
    src/DevicesPanel.cpp src/DevicesPanel.h
    src/Labels.cpp src/Labels.h
    src/LibraryPanel.cpp src/LibraryPanel.h
    src/ProjectView.cpp src/ProjectView.h
    src/SerialMonitor.cpp src/SerialMonitor.h
    src/SetupDialog.cpp src/SetupDialog.h
    src/SimulatorPanel.cpp src/SimulatorPanel.h
    src/MainWindow.cpp src/MainWindow.h
    src/MessageBar.cpp src/MessageBar.h
    src/Preferences.cpp src/Preferences.h
    src/CommandPalette.cpp src/CommandPalette.h
    src/ProblemsPanel.cpp src/ProblemsPanel.h
    src/Translations.cpp src/Translations.h
    src/Icons.cpp src/Icons.h
    src/TracePanel.cpp src/TracePanel.h
)
target_include_directories(esp32bench_ui PUBLIC src)
target_link_libraries(esp32bench_ui PUBLIC Qt6::Core Qt6::Network Qt6::Widgets)
target_compile_definitions(esp32bench_ui PUBLIC
    QT_NO_CAST_FROM_ASCII            # every user-visible string goes through tr()
    QT_NO_URL_CAST_FROM_STRING
    ESP32BENCH_UI_VERSION="${PROJECT_VERSION}"
)
if(MSVC)
    target_compile_options(esp32bench_ui PUBLIC /W3 /utf-8)
else()
    target_compile_options(esp32bench_ui PUBLIC -Wall -Wextra -Wpedantic)
endif()

# The UI's translations (Translations::languages()), compiled into the library as
# :/i18n/esp32bench_<lang>.qm. After changing UI strings, refresh the .ts files with
# `cmake --build build/ui --target update_translations` and translate what is new.
set(ESP32BENCH_TS_FILES
    translations/esp32bench_es.ts
    translations/esp32bench_de.ts
    translations/esp32bench_fr.ts
    translations/esp32bench_pt_BR.ts
)
qt_add_translations(esp32bench_ui
    TS_FILES ${ESP32BENCH_TS_FILES}
    RESOURCE_PREFIX "/i18n"
    LUPDATE_OPTIONS -locations none -no-obsolete
)

# Icons for systems without an icon theme (macOS, Windows): Icons::install().
qt_add_resources(esp32bench_ui "fallback_icons"
    PREFIX "/icons"
    BASE icons
    FILES
        icons/esp32bench-fallback/index.theme
        icons/esp32bench-fallback/scalable/application-exit.svg
        icons/esp32bench-fallback/scalable/application-menu.svg
        icons/esp32bench-fallback/scalable/configure.svg
        icons/esp32bench-fallback/scalable/debug-run.svg
        icons/esp32bench-fallback/scalable/debug-step-into.svg
        icons/esp32bench-fallback/scalable/debug-step-out.svg
        icons/esp32bench-fallback/scalable/debug-step-over.svg
        icons/esp32bench-fallback/scalable/dialog-error.svg
        icons/esp32bench-fallback/scalable/dialog-warning.svg
        icons/esp32bench-fallback/scalable/document-edit.svg
        icons/esp32bench-fallback/scalable/document-open-recent.svg
        icons/esp32bench-fallback/scalable/document-open-folder.svg
        icons/esp32bench-fallback/scalable/document-properties.svg
        icons/esp32bench-fallback/scalable/document-save.svg
        icons/esp32bench-fallback/scalable/document-send.svg
        icons/esp32bench-fallback/scalable/download.svg
        icons/esp32bench-fallback/scalable/edit-clear.svg
        icons/esp32bench-fallback/scalable/edit-copy.svg
        icons/esp32bench-fallback/scalable/edit-delete.svg
        icons/esp32bench-fallback/scalable/edit-paste.svg
        icons/esp32bench-fallback/scalable/edit-redo.svg
        icons/esp32bench-fallback/scalable/edit-undo.svg
        icons/esp32bench-fallback/scalable/folder-open.svg
        icons/esp32bench-fallback/scalable/go-down.svg
        icons/esp32bench-fallback/scalable/go-up.svg
        icons/esp32bench-fallback/scalable/help-about.svg
        icons/esp32bench-fallback/scalable/internet-web-browser.svg
        icons/esp32bench-fallback/scalable/list-add.svg
        icons/esp32bench-fallback/scalable/list-remove.svg
        icons/esp32bench-fallback/scalable/media-flash.svg
        icons/esp32bench-fallback/scalable/media-playback-pause.svg
        icons/esp32bench-fallback/scalable/media-record.svg
        icons/esp32bench-fallback/scalable/media-playback-start.svg
        icons/esp32bench-fallback/scalable/media-playback-stop.svg
        icons/esp32bench-fallback/scalable/network-serial.svg
        icons/esp32bench-fallback/scalable/preferences-system-network.svg
        icons/esp32bench-fallback/scalable/preferences-system.svg
        icons/esp32bench-fallback/scalable/process-stop.svg
        icons/esp32bench-fallback/scalable/run-build.svg
        icons/esp32bench-fallback/scalable/starred.svg
        icons/esp32bench-fallback/scalable/tools-report-bug.svg
        icons/esp32bench-fallback/scalable/utilities-terminal.svg
        icons/esp32bench-fallback/scalable/view-refresh.svg
        icons/esp32bench-fallback/scalable/view-restore.svg
        icons/esp32bench-fallback/scalable/window-close.svg
        icons/esp32bench-fallback/scalable/zoom-fit-best.svg
)

qt_add_executable(esp32bench-gui src/main.cpp)
# A GUI program on Windows (no console window); the macOS bundle is assembled by
# packaging/macos/build.sh.
set_target_properties(esp32bench-gui PROPERTIES WIN32_EXECUTABLE ON)
target_link_libraries(esp32bench-gui PRIVATE esp32bench_ui)
# The app icon, for the window when the installed theme icon is not there (dev builds).
qt_add_resources(esp32bench-gui "icon"
    PREFIX "/"
    BASE ../packaging/icons
    FILES ../packaging/icons/esp32bench.svg
)
install(TARGETS esp32bench-gui RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})

if(NOT BUILD_TESTING)
    return()
endif()
enable_testing()
foreach(t tst_rpcclient tst_devicespanel tst_librarypanel tst_projectview tst_build tst_serialmonitor tst_setup tst_board tst_simulator tst_debugger tst_display tst_pins tst_circuit tst_trace tst_translations tst_icons tst_accessibility tst_integration)
    qt_add_executable(${t} tests/${t}.cpp)
    target_link_libraries(${t} PRIVATE esp32bench_ui Qt6::Test)
    target_compile_definitions(${t} PRIVATE ESP32BENCH_SOURCE_DIR="${CMAKE_CURRENT_SOURCE_DIR}")
    add_test(NAME ${t} COMMAND ${t})
    set_tests_properties(${t} PROPERTIES ENVIRONMENT "QT_QPA_PLATFORM=offscreen")
endforeach()
