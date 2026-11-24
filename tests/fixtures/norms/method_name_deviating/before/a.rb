class Catalog
  def list_items
    items.map(&:name)
  end

  def item_count
    items.size
  end

  def find_item(id)
    items.find { |i| i.id == id }
  end
end
