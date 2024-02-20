cmake_minimum_required(VERSION 2.8)

project(tcpunchd CXX)

set(CMAKE_CXX_STANDARD 11)
set(CMAKE_CXX_STANDARD_REQUIRED True)

set(THREADS_PREFER_PTHREAD_FLAG ON)
find_package(Threads REQUIRED)
target_link_libraries(tcpunchd PRIVATE Threads::Threads)

add_executable(tcpunchd hole_punching_server.cpp)
