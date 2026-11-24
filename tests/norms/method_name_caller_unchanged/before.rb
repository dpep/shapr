class Catalog
  def list_items
    items.map(&:name)
  end

  def item_count
    items.size
  end

  def reload_items
    loadItems
  end

  private
end
