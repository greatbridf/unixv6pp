#ifndef OPEN_FILE_MANAGER_H
#define OPEN_FILE_MANAGER_H

#include "INode.h"
#include "File.h"
#include "FileSystem.h"

extern "C" File* OpenFileTable_f_alloc();
extern "C" void OpenFileTable_f_close(File*);

extern "C" Inode* InodeTable_get(short dev, int ino);
extern "C" void InodeTable_put(Inode*);
extern "C" bool InodeTable_is_loaded(short dev, int ino);
extern "C" void InodeTable_update();

#endif
