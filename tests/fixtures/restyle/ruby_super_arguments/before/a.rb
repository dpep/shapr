class Store < Base
  def save(record, options = {})
    log(record)
    super(record, options)
  end
end
