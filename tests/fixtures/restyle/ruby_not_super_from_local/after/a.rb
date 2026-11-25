class Store < Base
  def save(record)
    copy = record
    super
  end
end
