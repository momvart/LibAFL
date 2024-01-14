#include <stdint.h>
#include <stdlib.h>
#include <string>


#ifdef _MSC_VER 
#include <windows.h>

BOOL APIENTRY DllMain( HANDLE hModule, 
                   DWORD  ul_reason_for_call, 
                   LPVOID lpReserved
                 )
{
    return TRUE;
}

#define EXTERN __declspec(dllexport) extern "C"
#else 
#define EXTERN
extern "C" {
#endif

EXTERN int heap_uaf_read(const uint8_t *_data, size_t _size) {
  int *array = new int[100];
  delete[] array;
  fprintf(stdout, "%d\n", array[5]);
  return 0;
}

EXTERN int heap_uaf_write(const uint8_t *_data, size_t _size) {
  int *array = new int[100];
  delete[] array;
  array[5] = 1;
  return 0;
}

EXTERN int heap_oob_read(const uint8_t *_data, size_t _size) {
  int *array = new int[100];
  fprintf(stdout, "%d\n", array[100]);
  delete[] array;
  return 0;
}

EXTERN int heap_oob_write(const uint8_t *_data, size_t _size) {
  int *array = new int[100];
  array[100] = 1;
  delete[] array;
  return 0;
}
EXTERN int malloc_heap_uaf_read(const uint8_t *_data, size_t _size) {
  int *array = static_cast<int *>(malloc(100 * sizeof(int)));
  free(array);
  fprintf(stdout, "%d\n", array[5]);
  return 0;
}

EXTERN int malloc_heap_uaf_write(const uint8_t *_data, size_t _size) {
  int *array = static_cast<int *>(malloc(100 * sizeof(int)));
  free(array);
  array[5] = 1;
  return 0;
}

EXTERN int malloc_heap_oob_read(const uint8_t *_data, size_t _size) {
  int *array = static_cast<int *>(malloc(100 * sizeof(int)));
  fprintf(stdout, "%d\n", array[100]);
  free(array);
  return 0;
}

EXTERN int malloc_heap_oob_write(const uint8_t *_data, size_t _size) {
  int *array = static_cast<int *>(malloc(100 * sizeof(int)));
  array[100] = 1;
  free(array);
  return 0;
}

EXTERN int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
  // abort();
  return 0;
}

#ifndef _MSC_VER
}
#endif
