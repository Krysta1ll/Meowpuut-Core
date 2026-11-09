cmake_minimum_required(VERSION 3.16)
project(MeowpuutCore LANGUAGES CXX)

add_library(MeowpuutCore STATIC MeowpuutCore.cpp)
target_compile_features(MeowpuutCore PUBLIC cxx_std_17)
target_include_directories(MeowpuutCore PUBLIC "${CMAKE_CURRENT_SOURCE_DIR}")
if(MSVC)
    target_compile_options(MeowpuutCore PRIVATE /utf-8)
endif()
