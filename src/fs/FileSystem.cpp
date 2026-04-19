#include "FileSystem.h"
#include "Utility.h"

extern "C" bool FileSystem_load_super_block();
extern "C" bool FileSystem_is_readonly(short);
extern "C" void FileSystem_update();
extern "C" Inode* FileSystem_i_alloc(short);
extern "C" void FileSystem_i_free(short, int);
extern "C" Buf* FileSystem_alloc(short);
extern "C" void FileSystem_free(short, int);

/*==============================class FileSystem===================================*/
FileSystem::FileSystem()
{
	//nothing to do here
}

FileSystem::~FileSystem()
{
	//nothing to do here
}

void FileSystem::Initialize()
{
	//nothing to do here
}

void FileSystem::LoadSuperBlock()
{
	if (!FileSystem_load_super_block())
                Utility::Panic("Load SuperBlock Error....!\n");
}

bool FileSystem::IsReadOnly(short dev)
{
        return FileSystem_is_readonly(dev);
}

void FileSystem::Update()
{
	FileSystem_update();
}

Inode* FileSystem::IAlloc(short dev)
{
	return FileSystem_i_alloc(dev);
}

void FileSystem::IFree(short dev, int number)
{
	FileSystem_i_free(dev, number);
}

Buf* FileSystem::Alloc(short dev)
{
	return FileSystem_alloc(dev);
}

void FileSystem::Free(short dev, int blkno)
{
	FileSystem_free(dev, blkno);
}
