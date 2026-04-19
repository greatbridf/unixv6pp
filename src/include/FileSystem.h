#ifndef FILE_SYSTEM_H
#define FILE_SYSTEM_H

#include "INode.h"
#include "Buf.h"
#include "BufferManager.h"

#include <fs_defines.h>

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
	FileSystem();
	/* Destructors */
	~FileSystem();

	/*
	 * @comment 初始化成员变量
	 */
	void Initialize();

	/*
	* @comment 系统初始化时读入SuperBlock
	*/
	void LoadSuperBlock();

	/*
	 * @comment 检查文件系统是否只读
	 */
	bool IsReadOnly(short dev);

	/*
	 * @comment 将SuperBlock对象的内存副本更新到
	 * 存储设备的SuperBlock中去
	 */
	void Update();

	/*
	 * @comment  在存储设备dev上分配一个空闲
	 * 外存INode，一般用于创建新的文件。
	 */
	Inode* IAlloc(short dev);
	/*
	 * @comment  释放存储设备dev上编号为number
	 * 的外存INode，一般用于删除文件。
	 */
	void IFree(short dev, int number);

	/*
	 * @comment 在存储设备dev上分配空闲磁盘块
	 */
	Buf* Alloc(short dev);
	/*
	 * @comment 释放存储设备dev上编号为blkno的磁盘块
	 */
	void Free(short dev, int blkno);

private:
	// no members
};

#endif
