<?php

declare(strict_types=1);

namespace Acme\Widgets\Http;

use Acme\Widgets\Repository\ProductRepository;
use Psr\Http\Message\ResponseInterface;

interface Controller
{
    public function handle(array $request): ResponseInterface;
}

final class ProductController implements Controller
{
    private const PAGE_SIZE = 25;

    public function __construct(private ProductRepository $products)
    {
    }

    public function handle(array $request): ResponseInterface
    {
        $page = max(1, (int) ($request['page'] ?? 1));
        $items = $this->products->page($page, self::PAGE_SIZE);
        return $this->json(['page' => $page, 'items' => $items]);
    }

    private function json(array $payload): ResponseInterface
    {
        return new JsonResponse(json_encode($payload, JSON_THROW_ON_ERROR));
    }
}

function format_price(int $cents): string
{
    return sprintf('%d.%02d', intdiv($cents, 100), $cents % 100);
}
