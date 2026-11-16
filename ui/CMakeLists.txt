cmake_minimum_required(VERSION 3.24)
project(esp32bench-ui VERSION 0.0.1 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_AUTOMOC ON)

find_package(Qt6 6.5 REQUIRED COMPONENTS Core Network Widgets Test)
qt_standard_project_setup()

# Everything except main() goes in a library so the tests can link it.
add_library(esp32bench_ui STATIC
    src/RpcClient.cpp src/RpcClient.h
    src/BuildPanel.cpp src/BuildPanel.h
    src/CoreConnection.cpp src/CoreConnection.h
    src/DevicesPanel.cpp src/DevicesPanel.h
    src/Labels.cpp src/Labels.h
    src/LibraryPanel.cpp src/LibraryPanel.h
    src/ProjectView.cpp src/ProjectView.h
    src/SerialMonitor.cpp src/SerialMonitor.h
    src/MainWindow.cpp src/MainWindow.h
)
target_include_directories(esp32bench_ui PUBLIC src)
target_link_libraries(esp32bench_ui PUBLIC Qt6::Core Qt6::Network Qt6::Widgets)
target_compile_definitions(esp32bench_ui PUBLIC
    QT_NO_CAST_FROM_ASCII            # every user-visible string goes through tr()
    QT_NO_URL_CAST_FROM_STRING
    ESP32BENCH_UI_VERSION="${PROJECT_VERSION}"
)
target_compile_options(esp32bench_ui PUBLIC -Wall -Wextra -Wpedantic)

qt_add_executable(esp32bench-gui src/main.cpp)
target_link_libraries(esp32bench-gui PRIVATE esp32bench_ui)

enable_testing()
foreach(t tst_rpcclient tst_devicespanel tst_librarypanel tst_projectview tst_build tst_serialmonitor tst_integration)
    qt_add_executable(${t} tests/${t}.cpp)
    target_link_libraries(${t} PRIVATE esp32bench_ui Qt6::Test)
    target_compile_definitions(${t} PRIVATE ESP32BENCH_SOURCE_DIR="${CMAKE_CURRENT_SOURCE_DIR}")
    add_test(NAME ${t} COMMAND ${t})
    set_tests_properties(${t} PROPERTIES ENVIRONMENT "QT_QPA_PLATFORM=offscreen")
endforeach()
