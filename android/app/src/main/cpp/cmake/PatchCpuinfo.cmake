# ONNX Runtime fetches cpuinfo itself. This hook runs inside cpuinfo's
# project() after extraction and can run again on an incremental configure.
set(l2_patch "${CMAKE_CURRENT_LIST_DIR}/cpuinfo-arm-linux-l2.patch")
execute_process(
    COMMAND "${Patch_EXECUTABLE}" -R -f -p1 --dry-run -i "${l2_patch}"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    OUTPUT_QUIET ERROR_QUIET RESULT_VARIABLE already_applied
)
if(NOT already_applied EQUAL 0)
    execute_process(
        COMMAND "${Patch_EXECUTABLE}" -f -p1 -i "${l2_patch}"
        WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
        COMMAND_ERROR_IS_FATAL ANY
    )
endif()
