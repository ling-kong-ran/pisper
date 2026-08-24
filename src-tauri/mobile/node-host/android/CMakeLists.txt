cmake_minimum_required(VERSION 3.22.1)
project(pisper_node_host LANGUAGES CXX)

if(NOT ANDROID_ABI STREQUAL "arm64-v8a")
  return()
endif()

add_library(node SHARED IMPORTED)
set_target_properties(node PROPERTIES
  IMPORTED_LOCATION "${CMAKE_CURRENT_SOURCE_DIR}/../../jniLibs/${ANDROID_ABI}/libnode.so"
)

add_library(pisper_node_host SHARED node_host.cpp)
target_compile_features(pisper_node_host PRIVATE cxx_std_20)
target_include_directories(pisper_node_host PRIVATE
  "${CMAKE_CURRENT_SOURCE_DIR}/../node-mobile/include/node"
)
# Android 15+ 的 16 KB 页设备要求所有 ELF LOAD 段至少按 16 KB 对齐。
target_link_options(pisper_node_host PRIVATE "-Wl,-z,max-page-size=16384")
target_link_libraries(pisper_node_host PRIVATE node log android)
