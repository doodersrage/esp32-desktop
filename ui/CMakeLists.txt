cmake_minimum_required(VERSION 3.24)
project(esp32bench-ui VERSION 0.1.5 LANGUAGES CXX)

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
    src/ConsoleView.cpp src/ConsoleView.h
    src/CoreConnection.cpp src/CoreConnection.h
    src/DebugPanel.cpp src/DebugPanel.h
    src/DisplayPanel.cpp src/DisplayPanel.h
    src/PinsPanel.cpp src/PinsPanel.h
    src/CircuitPanel.cpp src/CircuitPanel.h
    src/DevicesPanel.cpp src/DevicesPanel.h
    src/Labels.cpp src/Labels.h
    src/LibraryPanel.cpp src/LibraryPanel.h
    src/ProjectView.cpp src/ProjectView.h
    src/SerialMonitor.cpp src/SerialMonitor.h
    src/SetupDialog.cpp src/SetupDialog.h
    src/SimulatorPanel.cpp src/SimulatorPanel.h
    src/MainWindow.cpp src/MainWindow.h
    src/Translations.cpp src/Translations.h
)
target_include_directories(esp32bench_ui PUBLIC src)
target_link_libraries(esp32bench_ui PUBLIC Qt6::Core Qt6::Network Qt6::Widgets)
target_compile_definitions(esp32bench_ui PUBLIC
    QT_NO_CAST_FROM_ASCII            # every user-visible string goes through tr()
    QT_NO_URL_CAST_FROM_STRING
    ESP32BENCH_UI_VERSION="${PROJECT_VERSION}"
)
target_compile_options(esp32bench_ui PUBLIC -Wall -Wextra -Wpedantic)

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

qt_add_executable(esp32bench-gui src/main.cpp)
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
foreach(t tst_rpcclient tst_devicespanel tst_librarypanel tst_projectview tst_build tst_serialmonitor tst_setup tst_board tst_simulator tst_debugger tst_display tst_pins tst_circuit tst_translations tst_integration)
    qt_add_executable(${t} tests/${t}.cpp)
    target_link_libraries(${t} PRIVATE esp32bench_ui Qt6::Test)
    target_compile_definitions(${t} PRIVATE ESP32BENCH_SOURCE_DIR="${CMAKE_CURRENT_SOURCE_DIR}")
    add_test(NAME ${t} COMMAND ${t})
    set_tests_properties(${t} PROPERTIES ENVIRONMENT "QT_QPA_PLATFORM=offscreen")
endforeach()
