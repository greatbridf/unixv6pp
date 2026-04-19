#ifndef FILE_SYSTEM_H
#define FILE_SYSTEM_H

#include "INode.h"
#include "Buf.h"
#include "BufferManager.h"
#include "Utility.h"

#include <fs_defines.h>

extern "C" bool FileSystem_load_super_block();
extern "C" bool FileSystem_is_readonly(short);
extern "C" void FileSystem_update();
extern "C" Inode* FileSystem_i_alloc(short);
extern "C" void FileSystem_i_free(short, int);
extern "C" Buf* FileSystem_alloc(short);
extern "C" void FileSystem_free(short, int);

/*
 * 文件系统类(FileSystem)管理文件存储设备中
 * 的各类存储资源，磁盘块、外存INode的分配、
 * 释放。
 */
class FileSystem
{
public:
	/* static consts */
	static const int NMOUNT = 5;			/* 系统中用于挂载子文件系统的装配块数量 */
	static const int ROOTINO = 1;			/* 文件系统根目录外存Inode编号 */

	static const unsigned long INODE_SIZE = 64;
	static const unsigned long INODE_NUMBER_PER_SECTOR = fs::SECTOR_SIZE / INODE_SIZE;

	/* Functions */
public:
	/* Constructors */
	inline FileSystem() {}
	/* Destructors */
	inline ~FileSystem() {}

	/*
	 * @comment 初始化成员变量
	 */
	inline void Initialize()
	{
		//nothing to do here
	}

	/*
	* @comment 系统初始化时读入SuperBlock
	*/
	inline void LoadSuperBlock()
	{
		if (!FileSystem_load_super_block())
			Utility::Panic("Load SuperBlock Error....!\n");
	}

	/*
	 * @comment 检查文件系统是否只读
	 */
	inline bool IsReadOnly(short dev)
	{
		return FileSystem_is_readonly(dev);
	}

	/*
	 * @comment 将SuperBlock对象的内存副本更新到
	 * 存储设备的SuperBlock中去
	 */
	inline void Update()
	{
		FileSystem_update();
	}

	/*
	 * @comment  在存储设备dev上分配一个空闲
	 * 外存INode，一般用于创建新的文件。
	 */
	inline Inode* IAlloc(short dev)
	{
		return FileSystem_i_alloc(dev);
	}
	/*
	 * @comment  释放存储设备dev上编号为number
	 * 的外存INode，一般用于删除文件。
	 */
	inline void IFree(short dev, int number)
	{
		FileSystem_i_free(dev, number);
	}

	/*
	 * @comment 在存储设备dev上分配空闲磁盘块
	 */
	inline Buf* Alloc(short dev)
	{
		return FileSystem_alloc(dev);
	}
	/*
	 * @comment 释放存储设备dev上编号为blkno的磁盘块
	 */
	inline void Free(short dev, int blkno)
	{
		FileSystem_free(dev, blkno);
	}

private:
	// no members
};

#endif
