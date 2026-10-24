#import <UIKit/UIKit.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>
#import <Photos/Photos.h>
#import <PhotosUI/PhotosUI.h>
#include <stdbool.h>

@interface CalibRawSceneDelegate : UIResponder <UIWindowSceneDelegate>
@property(nonatomic, strong) UIWindow *window;
@end

@implementation CalibRawSceneDelegate

- (void)sceneDidBecomeActive:(UIScene *)scene {
    [NSNotificationCenter.defaultCenter
        postNotificationName:UIApplicationDidBecomeActiveNotification
        object:UIApplication.sharedApplication];
}

- (void)sceneWillResignActive:(UIScene *)scene {
    [NSNotificationCenter.defaultCenter
        postNotificationName:UIApplicationWillResignActiveNotification
        object:UIApplication.sharedApplication];
}

- (void)sceneWillEnterForeground:(UIScene *)scene {
    [NSNotificationCenter.defaultCenter
        postNotificationName:UIApplicationWillEnterForegroundNotification
        object:UIApplication.sharedApplication];
}

- (void)sceneDidEnterBackground:(UIScene *)scene {
    [NSNotificationCenter.defaultCenter
        postNotificationName:UIApplicationDidEnterBackgroundNotification
        object:UIApplication.sharedApplication];
}

@end

static UIWindow *calibraw_ios_window(void) {
    for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
        if ([scene.delegate isKindOfClass:[CalibRawSceneDelegate class]]) {
            UIWindow *window = ((CalibRawSceneDelegate *)scene.delegate).window;
            if (window) {
                return window;
            }
        }
    }
    return nil;
}

typedef void (*CalibRawDocumentCallback)(void *, const char *);

// A new folder for one import's files. Rust removes it, with everything in it,
// once the import finishes (`IosImportStaging`), so its name and parent must
// match what that type accepts.
static NSURL *calibraw_ios_create_import_staging(NSError **error) {
    NSURL *staging = [[NSURL fileURLWithPath:NSTemporaryDirectory() isDirectory:YES]
        URLByAppendingPathComponent:[@"CalibRaw-import-" stringByAppendingString:NSUUID.UUID.UUIDString]
        isDirectory:YES];
    return [NSFileManager.defaultManager createDirectoryAtURL:staging withIntermediateDirectories:YES
        attributes:nil error:error] ? staging : nil;
}

@interface CalibRawDocumentPickerDelegate : NSObject <UIDocumentPickerDelegate, PHPickerViewControllerDelegate, UIAdaptivePresentationControllerDelegate>
@property(nonatomic) void *context;
@property(nonatomic) CalibRawDocumentCallback callback;
@property(nonatomic) bool completed;
@property(nonatomic) bool loading;
@property(nonatomic, weak) UIPresentationController *activePresentation;
- (void)finish:(NSDictionary *)result;
- (void)presentFiles:(UIViewController *)presenter;
- (void)presentPhotos:(UIViewController *)presenter;
- (void)loadPhotos:(NSArray<PHPickerResult *> *)results;
@end

static CalibRawDocumentPickerDelegate *activeDocumentPicker;

@implementation CalibRawDocumentPickerDelegate

- (void)finish:(NSDictionary *)result {
    if (self.completed) {
        return;
    }
    self.completed = true;
    NSData *data = [NSJSONSerialization dataWithJSONObject:result options:0 error:nil];
    NSString *json = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
    self.callback(self.context, json ? json.UTF8String : "{\"error\":\"Could not encode picker result\"}");
    self.context = NULL;
    if (activeDocumentPicker == self) {
        activeDocumentPicker = nil;
    }
}

- (void)documentPicker:(UIDocumentPickerViewController *)controller didPickDocumentsAtURLs:(NSArray<NSURL *> *)urls {
    // presentFiles opens documents as copies, so every URL is this app's own
    // file in its tmp Inbox. Move them into the import staging folder; one that
    // cannot be staged is deleted, so no copy outlives the import.
    NSFileManager *files = NSFileManager.defaultManager;
    NSError *stagingError = nil;
    NSURL *staging = calibraw_ios_create_import_staging(&stagingError);
    if (!staging) {
        for (NSURL *url in urls) {
            [files removeItemAtURL:url error:nil];
        }
        [self finish:@{@"error": stagingError.localizedDescription ?: @"Could not stage the selected files"}];
        return;
    }
    NSMutableArray<NSString *> *paths = [NSMutableArray array];
    NSMutableArray<NSString *> *errors = [NSMutableArray array];
    [urls enumerateObjectsUsingBlock:^(NSURL *url, NSUInteger index, BOOL *stop) {
        if (!url.isFileURL) {
            [errors addObject:@"The selected document is not a local file"];
            return;
        }
        // One folder per file keeps the provider's name even when two match.
        NSURL *folder = [staging URLByAppendingPathComponent:[NSString stringWithFormat:@"%lu", (unsigned long)index]
            isDirectory:YES];
        NSURL *destination = [folder URLByAppendingPathComponent:url.lastPathComponent];
        NSError *moveError = nil;
        if ([files createDirectoryAtURL:folder withIntermediateDirectories:YES attributes:nil error:&moveError]
            && [files moveItemAtURL:url toURL:destination error:&moveError]) {
            [paths addObject:destination.path];
        } else {
            [files removeItemAtURL:url error:nil];
            [errors addObject:[NSString stringWithFormat:@"%@: %@", url.lastPathComponent,
                moveError.localizedDescription ?: @"Could not stage the selected file"]];
        }
    }];
    [self finish:@{@"paths": paths, @"errors": errors, @"staging_directory": staging.path}];
}

- (void)documentPickerWasCancelled:(UIDocumentPickerViewController *)controller {
    [self finish:@{@"cancelled": @YES}];
}

- (void)presentationControllerDidDismiss:(UIPresentationController *)controller {
    if (controller == self.activePresentation && !self.loading) {
        [self finish:@{@"cancelled": @YES}];
    }
}

- (void)presentFiles:(UIViewController *)presenter {
    UIDocumentPickerViewController *picker = [[UIDocumentPickerViewController alloc]
        initForOpeningContentTypes:@[UTTypeData] asCopy:YES];
    picker.allowsMultipleSelection = YES;
    picker.shouldShowFileExtensions = YES;
    picker.delegate = self;
    [presenter presentViewController:picker animated:YES completion:^{
        self.activePresentation = picker.presentationController;
        picker.presentationController.delegate = self;
    }];
}

- (void)presentPhotos:(UIViewController *)presenter {
    PHPickerConfiguration *configuration = [PHPickerConfiguration new];
    configuration.filter = PHPickerFilter.imagesFilter;
    configuration.selectionLimit = 0;
    configuration.preferredAssetRepresentationMode = PHPickerConfigurationAssetRepresentationModeCurrent;
    PHPickerViewController *picker = [[PHPickerViewController alloc] initWithConfiguration:configuration];
    picker.delegate = self;
    picker.modalPresentationStyle = UIModalPresentationFullScreen;
    [presenter presentViewController:picker animated:YES completion:^{
        self.activePresentation = picker.presentationController;
        picker.presentationController.delegate = self;
    }];
}

- (void)picker:(PHPickerViewController *)picker didFinishPicking:(NSArray<PHPickerResult *> *)results {
    self.loading = YES;
    [picker dismissViewControllerAnimated:YES completion:^{ [self loadPhotos:results]; }];
}

- (void)loadPhotos:(NSArray<PHPickerResult *> *)results {
    if (!results.count) {
        [self finish:@{@"cancelled": @YES}];
        return;
    }
    NSError *directoryError = nil;
    NSURL *staging = calibraw_ios_create_import_staging(&directoryError);
    if (!staging) {
        [self finish:@{@"error": directoryError.localizedDescription ?: @"Could not stage Photos import"}];
        return;
    }
    dispatch_group_t group = dispatch_group_create();
    NSMutableDictionary<NSNumber *, NSString *> *paths = [NSMutableDictionary dictionary];
    NSMutableDictionary<NSNumber *, NSString *> *errors = [NSMutableDictionary dictionary];
    [results enumerateObjectsUsingBlock:^(PHPickerResult *result, NSUInteger index, BOOL *stop) {
        NSItemProvider *provider = result.itemProvider;
        UTType *selectedType = nil;
        for (NSString *identifier in provider.registeredTypeIdentifiers) {
            UTType *type = [UTType typeWithIdentifier:identifier];
            if ([type conformsToType:UTTypeRAWImage]) {
                selectedType = type;
                break;
            }
            if (!selectedType && [type conformsToType:UTTypeImage]) {
                selectedType = type;
            }
        }
        if (!selectedType) {
            errors[@(index)] = @"Photos did not provide an image file representation";
            return;
        }
        dispatch_group_enter(group);
        [provider loadFileRepresentationForTypeIdentifier:selectedType.identifier completionHandler:^(NSURL *url, NSError *error) {
            NSString *failure = error.localizedDescription;
            NSString *path = nil;
            if (url && !error) {
                NSString *name = provider.suggestedName.lastPathComponent;
                if (!name.length) {
                    name = url.lastPathComponent;
                }
                UTType *nameType = [UTType typeWithFilenameExtension:name.pathExtension];
                BOOL raw = [selectedType conformsToType:UTTypeRAWImage];
                if (!name.pathExtension.length || ![nameType conformsToType:selectedType]) {
                    NSString *extension = selectedType.preferredFilenameExtension;
                    UTType *urlType = [UTType typeWithFilenameExtension:url.pathExtension];
                    if ([urlType conformsToType:selectedType]) {
                        extension = url.pathExtension;
                    }
                    if (extension.length) {
                        name = [name.stringByDeletingPathExtension stringByAppendingPathExtension:extension];
                    } else {
                        failure = @"Photos did not provide the image's file extension";
                    }
                }
                if (raw && ![[UTType typeWithFilenameExtension:name.pathExtension] conformsToType:UTTypeRAWImage]) {
                    failure = @"Photos did not provide a RAW file extension";
                }
                NSURL *folder = [staging URLByAppendingPathComponent:[NSString stringWithFormat:@"%lu", (unsigned long)index]
                    isDirectory:YES];
                NSError *copyError = nil;
                if (!failure && [NSFileManager.defaultManager createDirectoryAtURL:folder
                    withIntermediateDirectories:YES attributes:nil error:&copyError]) {
                    NSURL *destination = [folder URLByAppendingPathComponent:name];
                    if ([NSFileManager.defaultManager copyItemAtURL:url toURL:destination error:&copyError]) {
                        path = destination.path;
                    }
                }
                if (!path && !failure) {
                    failure = copyError.localizedDescription ?: @"Could not copy the selected photo";
                }
            }
            dispatch_async(dispatch_get_main_queue(), ^{
                if (path) {
                    paths[@(index)] = path;
                } else {
                    errors[@(index)] = failure ?: @"Photos did not provide the selected file";
                }
                dispatch_group_leave(group);
            });
        }];
    }];
    dispatch_group_notify(group, dispatch_get_main_queue(), ^{
        NSMutableArray<NSString *> *orderedPaths = [NSMutableArray array];
        NSMutableArray<NSString *> *orderedErrors = [NSMutableArray array];
        for (NSUInteger index = 0; index < results.count; index++) {
            if (paths[@(index)]) {
                [orderedPaths addObject:paths[@(index)]];
            }
            if (errors[@(index)]) {
                [orderedErrors addObject:errors[@(index)]];
            }
        }
        [self finish:@{@"paths": orderedPaths, @"errors": orderedErrors, @"staging_directory": staging.path}];
    });
}

@end

void calibraw_ios_open_documents(double anchorX, double anchorY, void *context, CalibRawDocumentCallback callback) {
    dispatch_async(dispatch_get_main_queue(), ^{
        CalibRawDocumentPickerDelegate *delegate = [CalibRawDocumentPickerDelegate new];
        delegate.context = context;
        delegate.callback = callback;
        if (activeDocumentPicker) {
            [delegate finish:@{@"error": @"A document picker is already open"}];
            return;
        }
        UIViewController *presenter = calibraw_ios_window().rootViewController;
        while (presenter.presentedViewController) {
            presenter = presenter.presentedViewController;
        }
        if (!presenter) {
            [delegate finish:@{@"error": @"No iOS window is available for the document picker"}];
            return;
        }
        activeDocumentPicker = delegate;
        UIAlertController *choice = [UIAlertController alertControllerWithTitle:nil message:nil
            preferredStyle:UIAlertControllerStyleActionSheet];
        __weak UIAlertController *weakChoice = choice;
        [choice addAction:[UIAlertAction actionWithTitle:@"Files" style:UIAlertActionStyleDefault handler:^(UIAlertAction *action) {
            delegate.activePresentation = nil;
            weakChoice.presentationController.delegate = nil;
            [presenter dismissViewControllerAnimated:YES completion:^{ [delegate presentFiles:presenter]; }];
        }]];
        [choice addAction:[UIAlertAction actionWithTitle:@"Photos" style:UIAlertActionStyleDefault handler:^(UIAlertAction *action) {
            delegate.activePresentation = nil;
            weakChoice.presentationController.delegate = nil;
            [presenter dismissViewControllerAnimated:YES completion:^{ [delegate presentPhotos:presenter]; }];
        }]];
        [choice addAction:[UIAlertAction actionWithTitle:@"Cancel" style:UIAlertActionStyleCancel handler:^(UIAlertAction *action) {
            [delegate finish:@{@"cancelled": @YES}];
        }]];
        choice.popoverPresentationController.sourceView = presenter.view;
        CGRect bounds = presenter.view.bounds;
        double positionX = anchorX >= 0 ? MIN(MAX(anchorX, 0), 1) * bounds.size.width
            : bounds.size.width - presenter.view.safeAreaInsets.right - 24;
        double positionY = anchorY >= 0 ? MIN(MAX(anchorY, 0), 1) * bounds.size.height
            : bounds.size.height - presenter.view.safeAreaInsets.bottom - 24;
        choice.popoverPresentationController.sourceRect = CGRectMake(
            bounds.origin.x + positionX, bounds.origin.y + positionY, 1, 1);
        [presenter presentViewController:choice animated:YES completion:^{
            delegate.activePresentation = choice.presentationController;
            choice.presentationController.delegate = delegate;
        }];
    });
}

void calibraw_ios_publish_photo(const char *path, const char *name, bool video,
                               void *context, CalibRawDocumentCallback callback) {
    NSString *filePath = [NSString stringWithUTF8String:path];
    NSString *filename = [NSString stringWithUTF8String:name];
    void (^finish)(NSString *) = ^(NSString *error) {
        NSDictionary *result = error ? @{ @"error": error } : @{ @"saved": @YES };
        NSData *data = [NSJSONSerialization dataWithJSONObject:result options:0 error:nil];
        NSString *json = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
        callback(context, json ? json.UTF8String : "{\"error\":\"Could not encode Photos result\"}");
    };
    dispatch_async(dispatch_get_main_queue(), ^{
        if (!filePath || !filename) {
            finish(@"Invalid photo export path or filename");
            return;
        }
        [PHPhotoLibrary requestAuthorizationForAccessLevel:PHAccessLevelAddOnly handler:^(PHAuthorizationStatus status) {
            if (status != PHAuthorizationStatusAuthorized) {
                finish(@"Photos saving is not allowed. Enable Add Photos Only access for CalibRaw in Settings.");
                return;
            }
            [PHPhotoLibrary.sharedPhotoLibrary performChanges:^{
                PHAssetCreationRequest *request = [PHAssetCreationRequest creationRequestForAsset];
                PHAssetResourceCreationOptions *options = [PHAssetResourceCreationOptions new];
                options.originalFilename = filename;
                options.shouldMoveFile = NO;
                [request addResourceWithType:video ? PHAssetResourceTypeVideo : PHAssetResourceTypePhoto
                    fileURL:[NSURL fileURLWithPath:filePath] options:options];
            } completionHandler:^(BOOL success, NSError *error) {
                finish(success ? nil : [NSString stringWithFormat:@"Could not save this image to Photos: %@",
                    error.localizedDescription ?: @"Photos rejected the image format"]);
            }];
        }];
    });
}

bool calibraw_ios_share_file(const char *path) {
    if (![NSThread isMainThread]) {
        return false;
    }
    NSString *filePath = [NSString stringWithUTF8String:path];
    UIViewController *presenter = calibraw_ios_window().rootViewController;
    while (presenter.presentedViewController) {
        presenter = presenter.presentedViewController;
    }
    if (!presenter || !filePath) {
        return false;
    }
    UIActivityViewController *share = [[UIActivityViewController alloc]
        initWithActivityItems:@[[NSURL fileURLWithPath:filePath]] applicationActivities:nil];
    share.popoverPresentationController.sourceView = presenter.view;
    share.popoverPresentationController.sourceRect = CGRectMake(
        CGRectGetMidX(presenter.view.bounds), CGRectGetMidY(presenter.view.bounds), 1, 1);
    [presenter presentViewController:share animated:YES completion:nil];
    return true;
}

bool calibraw_ios_attach_view(void *rawView) {
    UIView *view = (__bridge UIView *)rawView;
    UIWindow *window = view.window;
    if (!window) {
        return false;
    }
    for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
        if ([scene isKindOfClass:[UIWindowScene class]] &&
            [scene.session.role isEqualToString:UIWindowSceneSessionRoleApplication]) {
            UIWindowScene *windowScene = (UIWindowScene *)scene;
            window.windowScene = windowScene;
            window.frame = windowScene.coordinateSpace.bounds;
            if ([scene.delegate isKindOfClass:[CalibRawSceneDelegate class]]) {
                ((CalibRawSceneDelegate *)scene.delegate).window = window;
            }
            [window makeKeyAndVisible];
            return true;
        }
    }
    return false;
}

void calibraw_ios_system_bar_insets(double *insets) {
    for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
        if ([scene.delegate isKindOfClass:[CalibRawSceneDelegate class]]) {
            UIWindow *window = ((CalibRawSceneDelegate *)scene.delegate).window;
            if (window) {
                UIEdgeInsets safeArea = window.safeAreaInsets;
                double scale = window.screen.scale;
                insets[0] = safeArea.left * scale;
                insets[1] = safeArea.top * scale;
                insets[2] = safeArea.right * scale;
                insets[3] = safeArea.bottom * scale;
                return;
            }
        }
    }
}