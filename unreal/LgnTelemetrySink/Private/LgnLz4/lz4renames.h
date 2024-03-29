//
// injecting namespace emulation into lz4
//
#define LZ4_attach_dictionary LGN_LZ4_attach_dictionary
#define LZ4_compressBound LGN_LZ4_compressBound
#define LZ4_compress_default LGN_LZ4_compress_default
#define LZ4_compress_destSize LGN_LZ4_compress_destSize
#define LZ4_compress_fast LGN_LZ4_compress_fast
#define LZ4_compress_fast_continue LGN_LZ4_compress_fast_continue
#define LZ4_compress_fast_extState LGN_LZ4_compress_fast_extState
#define LZ4_compress_fast_extState_fastReset LGN_LZ4_compress_fast_extState_fastReset
#define LZ4_compress_forceExtDict LGN_LZ4_compress_forceExtDict
#define LZ4_createStream LGN_LZ4_createStream
#define LZ4_createStreamDecode LGN_LZ4_createStreamDecode
#define LZ4_decoderRingBufferSize LGN_LZ4_decoderRingBufferSize
#define LZ4_decompress_safe LGN_LZ4_decompress_safe
#define LZ4_decompress_safe_continue LGN_LZ4_decompress_safe_continue
#define LZ4_decompress_safe_forceExtDict LGN_LZ4_decompress_safe_forceExtDict
#define LZ4_decompress_safe_partial LGN_LZ4_decompress_safe_partial
#define LZ4_decompress_safe_usingDict LGN_LZ4_decompress_safe_usingDict
#define LZ4_freeStream LGN_LZ4_freeStream
#define LZ4_freeStreamDecode LGN_LZ4_freeStreamDecode
#define LZ4_initStream LGN_LZ4_initStream
#define LZ4_loadDict LGN_LZ4_loadDict
#define LZ4_resetStream LGN_LZ4_resetStream
#define LZ4_resetStream_fast LGN_LZ4_resetStream_fast
#define LZ4_saveDict LGN_LZ4_saveDict
#define LZ4_setStreamDecode LGN_LZ4_setStreamDecode
#define LZ4_sizeofState LGN_LZ4_sizeofState
#define LZ4_versionNumber LGN_LZ4_versionNumber
#define LZ4_versionString LGN_LZ4_versionString
#define LZ4_attach_HC_dictionary LGN_LZ4_attach_HC_dictionary
#define LZ4_compress_HC LGN_LZ4_compress_HC
#define LZ4_compress_HC_continue LGN_LZ4_compress_HC_continue
#define LZ4_compress_HC_continue_destSize LGN_LZ4_compress_HC_continue_destSize
#define LZ4_compress_HC_destSize LGN_LZ4_compress_HC_destSize
#define LZ4_compress_HC_extStateHC LGN_LZ4_compress_HC_extStateHC
#define LZ4_compress_HC_extStateHC_fastReset LGN_LZ4_compress_HC_extStateHC_fastReset
#define LZ4_createStreamHC LGN_LZ4_createStreamHC
#define LZ4_favorDecompressionSpeed LGN_LZ4_favorDecompressionSpeed
#define LZ4_freeStreamHC LGN_LZ4_freeStreamHC
#define LZ4_initStreamHC LGN_LZ4_initStreamHC
#define LZ4_loadDictHC LGN_LZ4_loadDictHC
#define LZ4_resetStreamHC LGN_LZ4_resetStreamHC
#define LZ4_resetStreamHC_fast LGN_LZ4_resetStreamHC_fast
#define LZ4_saveDictHC LGN_LZ4_saveDictHC
#define LZ4_setCompressionLevel LGN_LZ4_setCompressionLevel
#define LZ4_sizeofStateHC LGN_LZ4_sizeofStateHC
#define LZ4F_compressionLevel_max LGN_LZ4F_compressionLevel_max
#define LZ4F_compressFrameBound LGN_LZ4F_compressFrameBound
#define LZ4F_compressFrame LGN_LZ4F_compressFrame
#define LZ4F_getVersion LGN_LZ4F_getVersion
#define LZ4F_createCompressionContext LGN_LZ4F_createCompressionContext
#define LZ4F_freeCompressionContext LGN_LZ4F_freeCompressionContext
#define LZ4F_compressBegin LGN_LZ4F_compressBegin
#define LZ4F_compressBound LGN_LZ4F_compressBound
#define LZ4F_compressUpdate LGN_LZ4F_compressUpdate
#define LZ4F_flush LGN_LZ4F_flush
#define LZ4F_compressEnd LGN_LZ4F_compressEnd
#define LZ4F_createDecompressionContext LGN_LZ4F_createDecompressionContext
#define LZ4F_freeDecompressionContext LGN_LZ4F_freeDecompressionContext
#define LZ4F_headerSize LGN_LZ4F_headerSize
#define LZ4F_getFrameInfo LGN_LZ4F_getFrameInfo
#define LZ4F_decompress LGN_LZ4F_decompress
#define LZ4F_resetDecompressionContext LGN_LZ4F_resetDecompressionContext
